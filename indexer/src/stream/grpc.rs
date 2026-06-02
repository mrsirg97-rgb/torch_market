// Yellowstone gRPC subscription. Subscribes to BOTH torch_market and
// deep_pool program IDs through a single block filter (account_include
// matches txs touching any listed account, so listing both IDs delivers
// every tx for either program).
//
// Per-tx pipeline:
//   1. Walk meta.inner_instructions; decode each as a torch or deep_pool
//      event based on the inner ix's program_id_index.
//   2. Walk message.instructions; if the tx contains a memo program ix AND
//      at least one torch BondingCurveTrade, attach the memo text to that
//      trade. Memos without a torch trade are dropped (per docs/indexer.md
//      §"Messages: gating").
//
// On reconnect, resumes from `last_processed_slot - reorg_buffer`. Downstream
// idempotent UPSERTs de-dupe replays.

use std::collections::HashMap;

use anyhow::{Context, Result};
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest, SubscribeRequestFilterBlocks,
    SubscribeRequestPing,
};

use crate::constants::MEMO_PROGRAM_ID;
use crate::contracts::{AnyEvent, BlockBatch, DecodedEvent, DeepPoolEvent, TorchEvent};
use crate::stream::decoder::{
    try_decode_deep_pool_event, try_decode_torch_event, DeepPoolDiscriminators, TorchDiscriminators,
};

pub async fn run_subscriber(
    laserstream_url: String,
    laserstream_token: String,
    torch_program_id: String,
    deep_pool_program_id: String,
    resume_slot: u64,
    tx: mpsc::Sender<BlockBatch>,
) -> Result<()> {
    let torch_discs = TorchDiscriminators::compute();
    let deep_pool_discs = DeepPoolDiscriminators::compute();
    info!(
        torch = %torch_program_id,
        deep_pool = %deep_pool_program_id,
        resume_slot,
        "connecting to Laserstream"
    );

    let mut current_resume = resume_slot;

    loop {
        match subscribe_once(
            &laserstream_url,
            &laserstream_token,
            &torch_program_id,
            &deep_pool_program_id,
            current_resume,
            &torch_discs,
            &deep_pool_discs,
            &tx,
        )
        .await
        {
            Ok(()) => {
                warn!("gRPC stream ended cleanly; reconnecting");
            }
            Err(e) => {
                let chain = format!("{e:#}");
                if chain.contains("older than the oldest available slot") {
                    warn!(
                        previous_resume = current_resume,
                        "checkpoint older than Laserstream retention; falling back to current tip"
                    );
                    current_resume = 0;
                } else {
                    error!(error = chain, "gRPC stream errored; reconnecting in 2s");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn subscribe_once(
    url: &str,
    token: &str,
    torch_program_id: &str,
    deep_pool_program_id: &str,
    from_slot: u64,
    torch_discs: &TorchDiscriminators,
    deep_pool_discs: &DeepPoolDiscriminators,
    tx: &mpsc::Sender<BlockBatch>,
) -> Result<()> {
    let mut client = GeyserGrpcClient::build_from_shared(url.to_string())?
        .x_token(Some(token.to_string()))?
        .tls_config(ClientTlsConfig::new().with_native_roots())?
        .connect()
        .await
        .context("connect to Laserstream")?;

    let (mut subscribe_tx, mut stream) = client.subscribe().await?;

    // Reverted to BLOCKS filter — same pattern deep_pool and
    // metadao-challenge use on the same Helius creds, both working.
    // One filter entry per program; the `blocks` HashMap OR's entries
    // server-side. Two filter entries → deliver any block whose txs
    // touch torch OR deep_pool.
    let mut blocks = HashMap::new();
    blocks.insert(
        "torch_blocks".to_string(),
        SubscribeRequestFilterBlocks {
            account_include: vec![torch_program_id.to_string()],
            include_transactions: Some(true),
            include_accounts: Some(false),
            include_entries: Some(false),
        },
    );
    blocks.insert(
        "deep_pool_blocks".to_string(),
        SubscribeRequestFilterBlocks {
            account_include: vec![deep_pool_program_id.to_string()],
            include_transactions: Some(true),
            include_accounts: Some(false),
            include_entries: Some(false),
        },
    );

    let effective_from_slot = if from_slot > 0 { Some(from_slot) } else { None };

    let request = SubscribeRequest {
        blocks,
        commitment: Some(CommitmentLevel::Confirmed as i32),
        from_slot: effective_from_slot,
        ..Default::default()
    };

    subscribe_tx
        .send(request)
        .await
        .context("send subscribe request")?;

    info!(from_slot = ?effective_from_slot, "subscribed to blocks");

    let torch_bytes = bs58::decode(torch_program_id)
        .into_vec()
        .context("invalid torch_program_id")?;
    let deep_pool_bytes = bs58::decode(deep_pool_program_id)
        .into_vec()
        .context("invalid deep_pool_program_id")?;
    let memo_bytes = bs58::decode(MEMO_PROGRAM_ID)
        .into_vec()
        .context("invalid memo program id constant")?;

    while let Some(update) = stream.next().await {
        let update = update.context("recv stream update")?;
        match update.update_oneof {
            Some(UpdateOneof::Block(block)) => {
                let slot = block.slot;
                let block_time = block
                    .block_time
                    .and_then(|bt| chrono::DateTime::from_timestamp(bt.timestamp, 0));

                let tx_count = block.transactions.len();
                if tx_count > 0 {
                    debug!(slot, tx_count, "block received with txs");
                }

                let mut events = Vec::new();
                for tx_update in &block.transactions {
                    let Some(meta) = tx_update.meta.as_ref() else {
                        continue;
                    };
                    let Some(tx_info) = tx_update.transaction.as_ref() else {
                        continue;
                    };

                    let tx_events = decode_block_transaction(
                        tx_info,
                        meta,
                        slot,
                        block_time,
                        &torch_bytes,
                        &deep_pool_bytes,
                        &memo_bytes,
                        torch_discs,
                        deep_pool_discs,
                    );
                    events.extend(tx_events);
                }

                let batch = BlockBatch { slot, events };
                if tx.send(batch).await.is_err() {
                    warn!("writer channel closed; exiting subscribe loop");
                    return Ok(());
                }
            }
            Some(UpdateOneof::Ping(_)) => {
                let _ = subscribe_tx
                    .send(SubscribeRequest {
                        ping: Some(SubscribeRequestPing { id: 1 }),
                        ..Default::default()
                    })
                    .await;
            }
            Some(UpdateOneof::Pong(_)) => {}
            Some(_) => {}
            None => {}
        }
    }

    Ok(())
}

// Per-tx decode. Pure function (modulo metrics counter increments inside
// log_decode_failure) so unit tests can drive it with synthetic
// `Transaction` / `TransactionStatusMeta` shapes.
//
// Returns the events decoded from this tx's inner instructions, with memo
// attribution applied (if a memo program ix is co-resident at the outer
// level AND at least one BondingCurveTrade event exists in the tx). Skips
// txs that don't actually invoke either program — references-as-accounts
// (e.g., upgrade txs) don't carry events.
#[allow(clippy::too_many_arguments)]
pub fn decode_block_transaction(
    tx_info: &yellowstone_grpc_proto::prelude::Transaction,
    meta: &yellowstone_grpc_proto::prelude::TransactionStatusMeta,
    slot: u64,
    block_time: Option<chrono::DateTime<chrono::Utc>>,
    torch_program_bytes: &[u8],
    deep_pool_program_bytes: &[u8],
    memo_program_bytes: &[u8],
    torch_discs: &TorchDiscriminators,
    deep_pool_discs: &DeepPoolDiscriminators,
) -> Vec<DecodedEvent> {
    let signature = tx_info
        .signatures
        .first()
        .map(|s| bs58::encode(s).into_string())
        .unwrap_or_default();

    let account_keys = collect_account_keys(tx_info, meta);
    let torch_idx = account_keys
        .iter()
        .position(|k| k.as_slice() == torch_program_bytes);
    let deep_pool_idx = account_keys
        .iter()
        .position(|k| k.as_slice() == deep_pool_program_bytes);
    let memo_idx = account_keys
        .iter()
        .position(|k| k.as_slice() == memo_program_bytes);

    // Skip txs that reference either program only as an account (e.g.,
    // deploy/upgrade txs). Either must actually be invoked.
    let invoked = is_invoked(tx_info, meta, torch_idx)
        || is_invoked(tx_info, meta, deep_pool_idx);
    if !invoked {
        return Vec::new();
    }

    // Memo text from the outer-instruction list. The SDK's addMemoIx puts
    // it there, not in inner_instructions.
    let memo_text = memo_idx.and_then(|m_idx| {
        tx_info.message.as_ref().and_then(|msg| {
            msg.instructions
                .iter()
                .find(|ix| ix.program_id_index as usize == m_idx)
                .map(|ix| String::from_utf8_lossy(&ix.data).to_string())
        })
    });

    let mut tx_events: Vec<DecodedEvent> = Vec::new();
    let mut flat_idx: i32 = 0;
    for inner in &meta.inner_instructions {
        // Resolve whether the outer instruction that emitted this group's
        // events is a `*_via_vault` leverage variant — drives `owner_is_vault`.
        // `inner.index` points at the outer ix in the top-level message.
        let via_vault = tx_info
            .message
            .as_ref()
            .and_then(|msg| msg.instructions.get(inner.index as usize))
            .map(|outer| {
                Some(outer.program_id_index as usize) == torch_idx
                    && torch_discs.is_via_vault_ix(&outer.data)
            })
            .unwrap_or(false);
        for ix in &inner.instructions {
            let pidx = ix.program_id_index as usize;
            if Some(pidx) == torch_idx {
                match try_decode_torch_event(&ix.data, torch_discs) {
                    Ok(event) => {
                        tx_events.push(DecodedEvent {
                            signature: signature.clone(),
                            inner_ix_idx: flat_idx,
                            slot: slot as i64,
                            block_time,
                            event: AnyEvent::Torch(event),
                            memo: None,
                            via_vault,
                        });
                    }
                    Err(e) => log_decode_failure(slot, &signature, ix, "torch", e),
                }
            } else if Some(pidx) == deep_pool_idx {
                match try_decode_deep_pool_event(&ix.data, deep_pool_discs) {
                    Ok(event) => {
                        tx_events.push(DecodedEvent {
                            signature: signature.clone(),
                            inner_ix_idx: flat_idx,
                            slot: slot as i64,
                            block_time,
                            event: AnyEvent::DeepPool(event),
                            memo: None,
                            via_vault: false,
                        });
                    }
                    Err(e) => log_decode_failure(slot, &signature, ix, "deep_pool", e),
                }
            }
            flat_idx += 1;
        }
    }

    // Memo attribution: attach to the FIRST trade-like event in the tx —
    // either a BondingCurveTrade (pre-migration) or a deep_pool SwapExecuted
    // (post-migration). Either qualifies the tx as a torch-token trade, so
    // memos attached to either should be surfaced as messages. Memos without
    // any trade context are dropped per the gating policy.
    if let Some(memo) = memo_text {
        let mut attached = false;
        for de in tx_events.iter_mut() {
            let is_trade = matches!(
                &de.event,
                AnyEvent::Torch(TorchEvent::BondingCurveTrade(_))
                    | AnyEvent::DeepPool(DeepPoolEvent::SwapExecuted(_))
            );
            if is_trade {
                de.memo = Some(memo.clone());
                attached = true;
                break;
            }
        }
        if !attached {
            debug!(
                slot,
                sig = %signature,
                "memo present but no trade event; dropping per gating policy"
            );
        }
    }

    tx_events
}

// Returns true if the given program index actually shows up as the
// program_id_index of an outer or inner instruction (i.e., the program was
// invoked, not just referenced as an account).
fn is_invoked(
    tx_info: &yellowstone_grpc_proto::prelude::Transaction,
    meta: &yellowstone_grpc_proto::prelude::TransactionStatusMeta,
    program_idx: Option<usize>,
) -> bool {
    let Some(program_idx) = program_idx else {
        return false;
    };
    let outer = tx_info
        .message
        .as_ref()
        .map(|msg| {
            msg.instructions
                .iter()
                .any(|ix| ix.program_id_index as usize == program_idx)
        })
        .unwrap_or(false);
    let inner = meta
        .inner_instructions
        .iter()
        .flat_map(|g| g.instructions.iter())
        .any(|ix| ix.program_id_index as usize == program_idx);
    outer || inner
}

fn log_decode_failure(
    slot: u64,
    signature: &str,
    ix: &yellowstone_grpc_proto::prelude::InnerInstruction,
    program: &str,
    e: crate::error::DecodeError,
) {
    // Silently skip non-event inner ixs — these are regular CPIs (e.g.,
    // torch program CPI'ing into deep_pool::create_pool, which carries
    // the create_pool instruction discriminator, NOT EVENT_IX_TAG_LE).
    // They're not decode failures, just instructions we don't care about.
    if matches!(e, crate::error::DecodeError::NotAnEvent) {
        return;
    }
    crate::metrics::METRICS
        .decode_errors_total
        .with_label_values(&[program])
        .inc();
    let prefix_hex: String = ix
        .data
        .iter()
        .take(16)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ");
    warn!(
        slot,
        sig = %signature,
        program,
        error = %e,
        data_len = ix.data.len(),
        first_16_bytes = %prefix_hex,
        "self-CPI did not decode as a known event"
    );
}

// Join static + loaded-writable + loaded-readonly account keys in the
// canonical order Solana uses for `program_id_index` resolution.
fn collect_account_keys(
    tx: &yellowstone_grpc_proto::prelude::Transaction,
    meta: &yellowstone_grpc_proto::prelude::TransactionStatusMeta,
) -> Vec<Vec<u8>> {
    let mut keys: Vec<Vec<u8>> = Vec::new();
    if let Some(msg) = tx.message.as_ref() {
        keys.extend(msg.account_keys.iter().cloned());
    }
    keys.extend(meta.loaded_writable_addresses.iter().cloned());
    keys.extend(meta.loaded_readonly_addresses.iter().cloned());
    keys
}
