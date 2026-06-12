// Historical backfill via Solana JSON-RPC.
//
// Walks `getSignaturesForAddress(program, { before: <oldest> })` backward
// page by page (1000-sig max per RPC limit), fetches each tx via
// getTransaction, decodes events through the SAME try_decode_*_event paths
// live ingest uses, and inserts via the domain layer.
//
// Two passes: deep_pool first, then torch_market. Order matters because
// torch's MigratedToDex sets `markets.deep_pool_pubkey` (FK to pools.pubkey)
// — walking deep_pool first guarantees the referenced pool exists before
// torch sees the migration event.
//
// Independent of the live subscriber — runs as a one-shot subcommand and
// exits. Never touches `indexer_state.last_processed_slot` (that's the
// live indexer's checkpoint; must not regress).
//
// `START_SLOT` env var (parsed by config::Config) acts as a lower bound:
// any page whose oldest sig falls below `start_slot` short-circuits
// further pagination.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use chrono::DateTime;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tracing::{info, warn};

use crate::contracts::{AnyEvent, DecodedEvent};
use crate::stream::decoder::{
    try_decode_deep_pool_event, try_decode_torch_event, DeepPoolDiscriminators, TorchDiscriminators,
};

const PAGE_LIMIT: usize = 1000;
const DEFAULT_TX_THROTTLE_MS: u64 = 100;
const MAX_HTTP_ATTEMPTS: usize = 4;

#[derive(Deserialize, Debug)]
struct SignatureEntry {
    signature: String,
    slot: u64,
    #[serde(rename = "blockTime")]
    block_time: Option<i64>,
}

#[derive(Copy, Clone)]
enum ProgramKind {
    DeepPool,
    Torch,
}

impl ProgramKind {
    fn name(&self) -> &'static str {
        match self {
            ProgramKind::DeepPool => "deep_pool",
            ProgramKind::Torch => "torch_market",
        }
    }
}

pub async fn run(
    rpc_url: String,
    torch_program_id: String,
    deep_pool_program_id: String,
    start_slot: Option<u64>,
    db: PgPool,
) -> Result<()> {
    let throttle_ms = std::env::var("BACKFILL_TX_THROTTLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TX_THROTTLE_MS);
    let throttle = std::time::Duration::from_millis(throttle_ms);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build reqwest client")?;

    let torch_discs = TorchDiscriminators::compute();
    let deep_pool_discs = DeepPoolDiscriminators::compute();

    // NB: rpc_url carries the API key inline — log only the HOST. The full
    // URL leaked to Cloud Logging once (2026-06-12); key rotated after.
    let rpc_host = rpc_url.split('?').next().unwrap_or("<unparseable>");
    info!(
        %torch_program_id,
        %deep_pool_program_id,
        rpc_host,
        throttle_ms,
        start_slot = ?start_slot,
        "starting backfill"
    );

    // Pass 1: deep_pool (so torch's MigratedToDex FK lands successfully).
    walk_program(
        &client,
        &rpc_url,
        &deep_pool_program_id,
        ProgramKind::DeepPool,
        throttle,
        start_slot,
        &db,
        &torch_discs,
        &deep_pool_discs,
    )
    .await
    .context("backfill deep_pool")?;

    // Pass 2: torch_market.
    walk_program(
        &client,
        &rpc_url,
        &torch_program_id,
        ProgramKind::Torch,
        throttle,
        start_slot,
        &db,
        &torch_discs,
        &deep_pool_discs,
    )
    .await
    .context("backfill torch_market")?;

    info!("backfill complete");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn walk_program(
    client: &reqwest::Client,
    rpc_url: &str,
    program_id: &str,
    kind: ProgramKind,
    throttle: std::time::Duration,
    start_slot: Option<u64>,
    db: &PgPool,
    torch_discs: &TorchDiscriminators,
    deep_pool_discs: &DeepPoolDiscriminators,
) -> Result<()> {
    info!(program = kind.name(), %program_id, "walking signatures");

    let mut before: Option<String> = None;
    let mut total_sigs = 0usize;
    let mut total_events = 0usize;
    let mut page_num = 0usize;

    loop {
        page_num += 1;
        let sigs = get_signatures(client, rpc_url, program_id, before.as_deref())
            .await
            .with_context(|| format!("fetch signatures page {page_num}"))?;
        if sigs.is_empty() {
            info!(
                program = kind.name(),
                page_num, "empty page; reached start of program history"
            );
            break;
        }

        let oldest_sig = sigs.last().unwrap().signature.clone();
        let newest_slot = sigs.first().unwrap().slot;
        let oldest_slot = sigs.last().unwrap().slot;
        info!(
            program = kind.name(),
            page_num,
            page_size = sigs.len(),
            slot_range = format!("{newest_slot} → {oldest_slot}"),
            "fetched page"
        );
        total_sigs += sigs.len();

        // Page-level start_slot cutoff: if the page's NEWEST sig is already
        // below the cutoff, no remaining sig is worth decoding; bail.
        if let Some(cutoff) = start_slot {
            if newest_slot < cutoff {
                info!(
                    program = kind.name(),
                    cutoff, newest_slot, "page entirely below START_SLOT; stopping"
                );
                break;
            }
        }

        let mut decoded: Vec<DecodedEvent> = Vec::new();
        // [I-1] Per-slot transaction ordinal: the reversed signature walk is
        // chain order, so a per-slot counter reproduces in-block position.
        // (A slot split across page boundaries restarts its counter — rare,
        // and the idempotent unique keys make replays safe; documented.)
        let mut slot_ordinals: HashMap<i64, i32> = HashMap::new();
        for entry in sigs.iter().rev() {
            // Per-sig cutoff: skip entries below START_SLOT but keep paging
            // since older sigs in *next* pages could still be relevant.
            if let Some(cutoff) = start_slot {
                if entry.slot < cutoff {
                    continue;
                }
            }
            match decode_tx(client, rpc_url, entry, program_id, kind, torch_discs, deep_pool_discs)
                .await
            {
                Ok(events) => {
                    let ord = slot_ordinals.entry(entry.slot as i64).or_insert(0);
                    let idx = *ord;
                    *ord += 1;
                    decoded.extend(events.into_iter().map(|mut e| {
                        e.tx_idx = idx;
                        e
                    }));
                }
                Err(e) => {
                    warn!(sig = %entry.signature, error = %e, "decode failed; skipping");
                }
            }
            if !throttle.is_zero() {
                tokio::time::sleep(throttle).await;
            }
        }
        total_events += decoded.len();

        let inserted = apply_page(db, decoded).await?;
        info!(
            program = kind.name(),
            page_num, new_in_page = inserted, "page processed"
        );

        // Stop conditions: empty page above; "zero new inserts" means we
        // caught up to live state (idempotent UPSERT noop).
        if inserted == 0 {
            info!(
                program = kind.name(),
                "page produced no new inserts; backfill caught up"
            );
            break;
        }

        before = Some(oldest_sig);
    }

    info!(
        program = kind.name(),
        total_pages = page_num,
        total_sigs,
        total_events,
        "program walk complete"
    );
    Ok(())
}

// Run one page worth of DecodedEvents through the same writer logic the live
// stream uses, minus the `indexer_state` checkpoint update. Reuses the
// per-event handling from stream/writer.rs by calling its public helpers.
async fn apply_page(db: &PgPool, events: Vec<DecodedEvent>) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }

    // The writer's write_block sorts events by (sig, inner_ix_idx). Here we
    // also have multi-slot events in one page, so add slot as the primary
    // sort key for deterministic per-page ordering.
    let mut events = events;
    // [I-1] chain order: slot, then in-block tx position, then inner ix.
    events.sort_by(|a, b| {
        a.slot
            .cmp(&b.slot)
            .then(a.tx_idx.cmp(&b.tx_idx))
            .then(a.inner_ix_idx.cmp(&b.inner_ix_idx))
            .then(a.signature.cmp(&b.signature))
    });

    // Group by slot, then feed each slot's events through write_block_no_checkpoint.
    let mut groups: HashMap<i64, Vec<DecodedEvent>> = HashMap::new();
    for de in events {
        groups.entry(de.slot).or_default().push(de);
    }

    let mut slots: Vec<i64> = groups.keys().copied().collect();
    slots.sort();

    let mut inserted_total = 0usize;
    for slot in slots {
        let evs = groups.remove(&slot).unwrap_or_default();
        inserted_total += crate::stream::writer::write_events_no_checkpoint(db, slot as u64, evs)
            .await
            .with_context(|| format!("apply_page slot {slot}"))?;
    }
    Ok(inserted_total)
}

async fn get_signatures(
    client: &reqwest::Client,
    rpc_url: &str,
    program_id: &str,
    before: Option<&str>,
) -> Result<Vec<SignatureEntry>> {
    let mut config = serde_json::Map::new();
    config.insert("limit".to_string(), json!(PAGE_LIMIT));
    if let Some(b) = before {
        config.insert("before".to_string(), json!(b));
    }
    config.insert("commitment".to_string(), json!("confirmed"));

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSignaturesForAddress",
        "params": [program_id, Value::Object(config)],
    });

    let resp: Value = rpc_call(client, rpc_url, &body, "getSignaturesForAddress").await?;
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("getSignaturesForAddress rpc error: {err}"));
    }
    let result = resp
        .get("result")
        .ok_or_else(|| anyhow!("getSignaturesForAddress: missing result"))?;
    serde_json::from_value(result.clone()).context("parse signatures result")
}

async fn rpc_call(
    client: &reqwest::Client,
    rpc_url: &str,
    body: &Value,
    method: &str,
) -> Result<Value> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=MAX_HTTP_ATTEMPTS {
        match client.post(rpc_url).json(body).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return resp
                        .json::<Value>()
                        .await
                        .with_context(|| format!("{method} decode body"));
                }
                let snippet = resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect::<String>();
                let transient = status.as_u16() == 429 || status.is_server_error();
                let err = anyhow!("{method} HTTP {status}: {snippet}");
                if transient && attempt < MAX_HTTP_ATTEMPTS {
                    let backoff = std::time::Duration::from_millis(250 * attempt as u64);
                    warn!(method, attempt, ?backoff, %status, "transient RPC error; backing off");
                    tokio::time::sleep(backoff).await;
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }
            Err(e) => {
                let err = anyhow::Error::new(e).context(format!("post {method}"));
                if attempt < MAX_HTTP_ATTEMPTS {
                    warn!(method, attempt, "network error; retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(250 * attempt as u64))
                        .await;
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("{method}: exhausted retry budget")))
}

#[allow(clippy::too_many_arguments)]
async fn decode_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    entry: &SignatureEntry,
    program_id: &str,
    kind: ProgramKind,
    torch_discs: &TorchDiscriminators,
    deep_pool_discs: &DeepPoolDiscriminators,
) -> Result<Vec<DecodedEvent>> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [
            entry.signature,
            {
                "encoding": "json",
                "commitment": "confirmed",
                "maxSupportedTransactionVersion": 0,
            }
        ],
    });

    let resp: Value = rpc_call(client, rpc_url, &body, "getTransaction").await?;
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("getTransaction rpc error: {err}"));
    }
    let result = match resp.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return Ok(Vec::new()),
    };

    let block_time = entry.block_time.and_then(|bt| DateTime::from_timestamp(bt, 0));

    let meta = result.get("meta").ok_or_else(|| anyhow!("tx missing meta"))?;
    let transaction = result
        .get("transaction")
        .ok_or_else(|| anyhow!("tx missing transaction"))?;
    let message = transaction
        .get("message")
        .ok_or_else(|| anyhow!("tx missing message"))?;

    let mut keys: Vec<String> = message
        .get("accountKeys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing accountKeys"))?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if let Some(la) = meta.get("loadedAddresses") {
        if let Some(w) = la.get("writable").and_then(|v| v.as_array()) {
            keys.extend(w.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
        if let Some(r) = la.get("readonly").and_then(|v| v.as_array()) {
            keys.extend(r.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
    }

    let program_idx = match keys.iter().position(|k| k == program_id) {
        Some(i) => i,
        None => return Ok(Vec::new()),
    };

    let inner_groups = meta
        .get("innerInstructions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Outer (top-level) instructions, used to resolve the emitting ix for
    // leverage events (`*_via_vault` ⇒ owner_is_vault).
    let outer_ixs = message
        .get("instructions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut events = Vec::new();
    let mut flat_idx: i32 = 0;
    for group in &inner_groups {
        // The parent outer instruction for this inner group.
        let via_vault = matches!(kind, ProgramKind::Torch)
            && group
                .get("index")
                .and_then(|v| v.as_u64())
                .and_then(|gi| outer_ixs.get(gi as usize))
                .map(|outer| {
                    let outer_pid = outer
                        .get("programIdIndex")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(u64::MAX) as usize;
                    let is_torch = outer_pid == program_idx;
                    let outer_data = outer
                        .get("data")
                        .and_then(|v| v.as_str())
                        .and_then(|d| bs58::decode(d).into_vec().ok())
                        .unwrap_or_default();
                    is_torch && torch_discs.is_via_vault_ix(&outer_data)
                })
                .unwrap_or(false);
        let instructions = group
            .get("instructions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for ix in &instructions {
            let pid_idx = ix
                .get("programIdIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(u64::MAX) as usize;
            if pid_idx == program_idx {
                let data_b58 = ix.get("data").and_then(|v| v.as_str()).unwrap_or("");
                if let Ok(data) = bs58::decode(data_b58).into_vec() {
                    let decoded = match kind {
                        ProgramKind::DeepPool => {
                            try_decode_deep_pool_event(&data, deep_pool_discs)
                                .ok()
                                .map(AnyEvent::DeepPool)
                        }
                        ProgramKind::Torch => try_decode_torch_event(&data, torch_discs)
                            .ok()
                            .map(AnyEvent::Torch),
                    };
                    if let Some(event) = decoded {
                        events.push(DecodedEvent {
                            tx_idx: 0, // assigned by the page walk (per-slot ordinal)
                            signature: entry.signature.clone(),
                            inner_ix_idx: flat_idx,
                            slot: entry.slot as i64,
                            block_time,
                            event,
                            // Backfill doesn't attempt memo attribution.
                            // Live ingest captures memos going forward; old
                            // memos for migrated markets aren't critical to
                            // recover.
                            memo: None,
                            via_vault,
                        });
                    }
                }
            }
            flat_idx += 1;
        }
    }

    Ok(events)
}
