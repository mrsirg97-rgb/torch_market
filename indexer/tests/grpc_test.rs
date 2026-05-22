// gRPC-path tests for `decode_block_transaction`.
//
// Builds synthetic Yellowstone proto types (Transaction +
// TransactionStatusMeta) and verifies the per-tx walker:
//   - resolves account_keys correctly (static + loaded_writable + loaded_readonly),
//   - skips txs that reference either program only as an account,
//   - dispatches inner ixs to the correct program decoder,
//   - extracts memo text from the OUTER instruction list,
//   - attaches memo to the first BondingCurveTrade (and drops it otherwise),
//   - keeps `inner_ix_idx` flat across inner-instruction groups,
//   - silently skips garbage inner-ix data without dropping siblings.
//
// No DB, no Postgres, no Yellowstone server — pure protobuf-struct
// construction.

use borsh::BorshSerialize;
use chrono::{DateTime, TimeZone, Utc};
use yellowstone_grpc_proto::prelude::{
    CompiledInstruction, InnerInstruction, InnerInstructions, Message, Transaction,
    TransactionStatusMeta,
};

use torch_indexer::constants::{EVENT_IX_TAG_LE, MEMO_PROGRAM_ID};
use torch_indexer::contracts::{
    AnyEvent, BondingCurveTrade, DeepPoolEvent, MarketCreated, PoolCreated, TorchEvent,
};
use torch_indexer::stream::decoder::{
    event_discriminator, DeepPoolDiscriminators, TorchDiscriminators,
};
use torch_indexer::stream::grpc::decode_block_transaction;

// Deterministic program IDs for these tests. The actual base58 strings
// don't matter — only the raw bytes flow through decode_block_transaction.
const TORCH_PROGRAM: [u8; 32] = [0xab; 32];
const DEEP_POOL_PROGRAM: [u8; 32] = [0xcd; 32];

fn signer() -> [u8; 32] {
    [0x42; 32]
}

fn memo_program_bytes() -> Vec<u8> {
    bs58::decode(MEMO_PROGRAM_ID).into_vec().unwrap()
}

fn block_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap()
}

// Wrap an Anchor #[event] payload as the bytes Anchor's emit_cpi! would
// emit: EVENT_IX_TAG_LE ++ event_discriminator ++ borsh(payload).
fn wrap_event<T: BorshSerialize>(name: &str, payload: &T) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + 256);
    data.extend_from_slice(&EVENT_IX_TAG_LE);
    data.extend_from_slice(&event_discriminator(name));
    payload.serialize(&mut data).unwrap();
    data
}

fn pk(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = byte;
    out
}

// Build a tx that invokes `program_idx` at the outer level with a (mostly
// empty) instruction, plus optionally a memo outer ix with the given text.
fn build_tx(
    account_keys: Vec<Vec<u8>>,
    outer_program_idx: u32,
    memo_idx: Option<(u32, &str)>,
) -> Transaction {
    let mut outer = vec![CompiledInstruction {
        program_id_index: outer_program_idx,
        accounts: vec![],
        data: vec![],
    }];
    if let Some((idx, text)) = memo_idx {
        outer.push(CompiledInstruction {
            program_id_index: idx,
            accounts: vec![],
            data: text.as_bytes().to_vec(),
        });
    }
    Transaction {
        signatures: vec![vec![0xee; 64]],
        message: Some(Message {
            account_keys,
            instructions: outer,
            ..Default::default()
        }),
    }
}

fn build_meta(
    inner_groups: Vec<Vec<(u32, Vec<u8>)>>, // (program_id_index, data) per group
    loaded_writable: Vec<Vec<u8>>,
    loaded_readonly: Vec<Vec<u8>>,
) -> TransactionStatusMeta {
    let inner_instructions = inner_groups
        .into_iter()
        .enumerate()
        .map(|(i, ixs)| InnerInstructions {
            index: i as u32,
            instructions: ixs
                .into_iter()
                .map(|(pid, data)| InnerInstruction {
                    program_id_index: pid,
                    accounts: vec![],
                    data,
                    stack_height: None,
                })
                .collect(),
        })
        .collect();
    TransactionStatusMeta {
        loaded_writable_addresses: loaded_writable,
        loaded_readonly_addresses: loaded_readonly,
        inner_instructions,
        ..Default::default()
    }
}

fn torch_market_created_bytes(mint: u8) -> Vec<u8> {
    wrap_event(
        "MarketCreated",
        &MarketCreated {
            mint: pk(mint),
            creator: pk(0xaa),
            name: "Test".into(),
            symbol: "TST".into(),
            metadata_uri: String::new(),
            is_community_token: false,
            sol_target: 30_000_000_000,
            virtual_sol_reserves: 30_000_000_000,
            virtual_token_reserves: 1_073_000_000_000_000,
        },
    )
}

fn torch_bonding_trade_bytes(mint: u8) -> Vec<u8> {
    wrap_event(
        "BondingCurveTrade",
        &BondingCurveTrade {
            mint: pk(mint),
            trader: pk(0xbb),
            vault: [0u8; 32],
            is_buy: true,
            sol_in: 1_000_000_000,
            sol_out: 0,
            tokens_in: 0,
            tokens_out: 500_000_000_000_000,
            sol_to_treasury: 5_000_000,
            sol_to_creator: 0,
            protocol_fee: 5_000_000,
            virtual_sol_after: 31_000_000_000,
            virtual_token_after: 1_072_500_000_000_000,
            real_sol_after: 1_000_000_000,
            real_token_after: 799_500_000_000_000,
        },
    )
}

fn deep_pool_pool_created_bytes(pool: u8) -> Vec<u8> {
    wrap_event(
        "PoolCreated",
        &PoolCreated {
            pool: pk(pool),
            config: pk(99),
            token_mint: pk(1),
            lp_mint: pk(98),
            creator: pk(0xaa),
            sol_in_gross: 0,
            sol_in_net: 30_000_000_000,
            tokens_in_gross: 0,
            tokens_in_net: 1_073_000_000_000_000,
            sol_reserve_after: 30_000_000_000,
            token_reserve_after: 1_073_000_000_000_000,
            lp_supply_after: 28_274_333,
            lp_to_creator: 28_274_333,
            lp_locked: 0,
        },
    )
}

fn discs() -> (TorchDiscriminators, DeepPoolDiscriminators) {
    (
        TorchDiscriminators::compute(),
        DeepPoolDiscriminators::compute(),
    )
}

// ─── Happy paths ────────────────────────────────────────────────────────

#[test]
fn tx_with_only_torch_ix_decodes_torch_events() {
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    let tx = build_tx(keys.clone(), 1, None);
    let meta = build_meta(
        vec![vec![(1u32, torch_market_created_bytes(7))]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1);
    match &events[0].event {
        AnyEvent::Torch(TorchEvent::MarketCreated(m)) => {
            assert_eq!(m.mint, pk(7));
        }
        other => panic!("expected MarketCreated, got {other:?}"),
    }
    assert_eq!(events[0].slot, 100);
    assert_eq!(events[0].inner_ix_idx, 0);
    assert_eq!(events[0].memo, None);
}

#[test]
fn tx_with_only_deep_pool_ix_decodes_deep_pool_events() {
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), DEEP_POOL_PROGRAM.to_vec()];
    let tx = build_tx(keys, 1, None);
    let meta = build_meta(
        vec![vec![(1u32, deep_pool_pool_created_bytes(50))]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0].event,
        AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(_))
    ));
}

#[test]
fn tx_with_both_programs_dispatches_per_inner_ix() {
    // Single tx invoking both torch (migrate_to_dex) and deep_pool
    // (create_pool, via CPI) — common in the migration flow.
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![
        signer().to_vec(),
        TORCH_PROGRAM.to_vec(),
        DEEP_POOL_PROGRAM.to_vec(),
    ];
    let tx = build_tx(keys, 1, None);
    let meta = build_meta(
        vec![vec![
            (1u32, torch_market_created_bytes(1)), // torch
            (2u32, deep_pool_pool_created_bytes(50)), // deep_pool
        ]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].event, AnyEvent::Torch(_)));
    assert!(matches!(events[1].event, AnyEvent::DeepPool(_)));
    // Flat inner-ix indexing across the single group.
    assert_eq!(events[0].inner_ix_idx, 0);
    assert_eq!(events[1].inner_ix_idx, 1);
}

#[test]
fn account_keys_include_loaded_writable_and_readonly() {
    // Versioned txs load extra addresses via address-lookup-tables. The
    // canonical program_id_index order is static_keys ++ loaded_writable
    // ++ loaded_readonly. Put the torch program in loaded_readonly to
    // verify the walker concatenates correctly.
    let (torch_discs, deep_pool_discs) = discs();
    let static_keys = vec![signer().to_vec()]; // index 0
    let loaded_writable = vec![vec![0u8; 32]]; // index 1 (padding)
    let loaded_readonly = vec![TORCH_PROGRAM.to_vec()]; // index 2

    // Outer ix at the static-key program (signer here, just a placeholder).
    let tx = build_tx(static_keys, 0, None);
    let meta = build_meta(
        vec![vec![(2u32, torch_market_created_bytes(9))]],
        loaded_writable,
        loaded_readonly,
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1, "torch_idx resolved via loaded_readonly");
    match &events[0].event {
        AnyEvent::Torch(TorchEvent::MarketCreated(m)) => assert_eq!(m.mint, pk(9)),
        _ => panic!("wrong variant"),
    }
}

// ─── Memo gating + attribution ─────────────────────────────────────────

#[test]
fn memo_attaches_to_first_bonding_curve_trade() {
    let (torch_discs, deep_pool_discs) = discs();
    let memo_bytes = memo_program_bytes();
    let keys = vec![
        signer().to_vec(),
        TORCH_PROGRAM.to_vec(),
        memo_bytes.clone(),
    ];
    // Outer ix list: torch ix at index 1, memo ix at index 2.
    let tx = build_tx(keys.clone(), 1, Some((2, "gm wgmi")));
    let meta = build_meta(
        vec![vec![(1u32, torch_bonding_trade_bytes(7))]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_bytes,
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].memo, Some("gm wgmi".to_string()));
}

#[test]
fn memo_dropped_when_no_torch_trade_present() {
    // Memo + a deep_pool swap (not a torch trade) → memo dropped per the
    // gating policy. The swap event still lands.
    let (torch_discs, deep_pool_discs) = discs();
    let memo_bytes = memo_program_bytes();
    let keys = vec![
        signer().to_vec(),
        DEEP_POOL_PROGRAM.to_vec(),
        memo_bytes.clone(),
    ];
    let tx = build_tx(keys, 1, Some((2, "should be dropped")));
    let meta = build_meta(
        vec![vec![(1u32, deep_pool_pool_created_bytes(50))]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_bytes,
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].event, AnyEvent::DeepPool(_)));
    assert_eq!(events[0].memo, None, "no torch trade → no memo attribution");
}

#[test]
fn memo_only_attaches_to_first_trade_in_multi_trade_tx() {
    let (torch_discs, deep_pool_discs) = discs();
    let memo_bytes = memo_program_bytes();
    let keys = vec![
        signer().to_vec(),
        TORCH_PROGRAM.to_vec(),
        memo_bytes.clone(),
    ];
    let tx = build_tx(keys, 1, Some((2, "only first")));
    // Two BondingCurveTrade events in the same inner-ix group.
    let meta = build_meta(
        vec![vec![
            (1u32, torch_bonding_trade_bytes(7)),
            (1u32, torch_bonding_trade_bytes(8)),
        ]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_bytes,
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].memo, Some("only first".to_string()));
    assert_eq!(events[1].memo, None);
}

// ─── Negative paths ─────────────────────────────────────────────────────

#[test]
fn tx_that_only_references_program_as_account_is_skipped() {
    // Program is in account_keys but no instruction (outer or inner)
    // invokes it. E.g., an upgrade tx that names the program as the
    // upgrade target. Should return zero events.
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    // Outer ix is at index 0 (signer, not the program).
    let tx = build_tx(keys, 0, None);
    // No inner instructions.
    let meta = build_meta(vec![], vec![], vec![]);

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 0);
}

#[test]
fn garbage_inner_ix_data_skipped_without_dropping_siblings() {
    // Two inner ixs in the same group. First is junk (decode fails); second
    // is a valid torch event. The valid event must still land.
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    let tx = build_tx(keys, 1, None);
    let junk = vec![0xff; 32]; // wrong tag bytes → UnknownDiscriminator
    let meta = build_meta(
        vec![vec![
            (1u32, junk),
            (1u32, torch_market_created_bytes(3)),
        ]],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 1);
    match &events[0].event {
        AnyEvent::Torch(TorchEvent::MarketCreated(m)) => assert_eq!(m.mint, pk(3)),
        _ => panic!("wrong variant"),
    }
    // The junk ix occupied flat_idx 0; the valid event sits at flat_idx 1.
    assert_eq!(events[0].inner_ix_idx, 1);
}

#[test]
fn inner_ix_idx_is_flat_across_multiple_groups() {
    // Inner instructions arrive in groups (one group per top-level ix that
    // CPI'd). flat_idx must monotonically increase across groups, not reset.
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    let tx = build_tx(keys, 1, None);
    let meta = build_meta(
        vec![
            vec![(1u32, torch_market_created_bytes(1))],     // group 0, flat_idx=0
            vec![(1u32, torch_bonding_trade_bytes(2))],      // group 1, flat_idx=1
            vec![(1u32, torch_market_created_bytes(3))],     // group 2, flat_idx=2
        ],
        vec![],
        vec![],
    );

    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].inner_ix_idx, 0);
    assert_eq!(events[1].inner_ix_idx, 1);
    assert_eq!(events[2].inner_ix_idx, 2);
}

#[test]
fn block_time_propagated_to_decoded_events() {
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    let tx = build_tx(keys, 1, None);
    let meta = build_meta(
        vec![vec![(1u32, torch_market_created_bytes(1))]],
        vec![],
        vec![],
    );
    let bt = block_time();
    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(bt),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    assert_eq!(events[0].block_time, Some(bt));
}

#[test]
fn signature_extracted_to_base58() {
    let (torch_discs, deep_pool_discs) = discs();
    let keys = vec![signer().to_vec(), TORCH_PROGRAM.to_vec()];
    let tx = build_tx(keys, 1, None);
    let meta = build_meta(
        vec![vec![(1u32, torch_market_created_bytes(1))]],
        vec![],
        vec![],
    );
    let events = decode_block_transaction(
        &tx,
        &meta,
        100,
        Some(block_time()),
        &TORCH_PROGRAM,
        &DEEP_POOL_PROGRAM,
        &memo_program_bytes(),
        &torch_discs,
        &deep_pool_discs,
    );
    // We set signatures[0] = [0xee; 64]; b58 of that is deterministic.
    let expected = bs58::encode(vec![0xeeu8; 64]).into_string();
    assert_eq!(events[0].signature, expected);
}
