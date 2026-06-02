// Translation layer tests — event → DB insert shape mapping.
//
// Pure-function tests, no DB. Locks the field-by-field mapping so an
// accidental swap (e.g. real_sol vs virtual_sol) trips immediately.

use chrono::{DateTime, Utc};

use torch_indexer::contracts::{
    AnyEvent, BondingCurveTrade, DecodedEvent, DeepPoolEvent, MarketCreated, MarketStatus,
    MarketTier, MigratedToDex, OpenShortEvent, PoolCreated, PositionSide, TorchEvent,
};
use torch_indexer::stream::translate::{
    b58, new_market, new_migration, new_pool, new_position_open_short, new_trade, pool_pubkey,
    tier_from_target, torch_event_mint, ts, PUBKEY_DEFAULT,
};

fn pk(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = byte;
    out
}

fn pk_all(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn fixed_ts() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn de_for(event: AnyEvent, slot: i64, inner: i32) -> DecodedEvent {
    DecodedEvent {
        signature: format!("sig_{slot}_{inner}"),
        inner_ix_idx: inner,
        slot,
        block_time: Some(fixed_ts()),
        event,
        memo: None,
        via_vault: false,
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────

#[test]
fn b58_encodes_pubkey_array() {
    // All-zero pubkey base58 = 11111111111111111111111111111111
    assert_eq!(b58(&[0u8; 32]), "11111111111111111111111111111111");

    // Sanity: distinct first bytes yield distinct strings.
    let a = b58(&pk(1));
    let b = b58(&pk(2));
    assert_ne!(a, b);
    // ...and they're 32-44 chars (standard Solana base58 range).
    assert!(a.len() >= 32 && a.len() <= 44);
}

#[test]
fn ts_falls_back_to_now_when_block_time_missing() {
    let de = DecodedEvent {
        signature: "x".into(),
        inner_ix_idx: 0,
        slot: 1,
        block_time: None,
        event: AnyEvent::Torch(TorchEvent::TokenRevived(
            torch_indexer::contracts::TokenRevived {
                mint: pk(0),
                total_contributed: 0,
                revival_slot: 0,
            },
        )),
        memo: None,
        via_vault: false,
    };
    let before = Utc::now();
    let got = ts(&de);
    let after = Utc::now();
    // ts() returned a sane "now" — between the two clock reads.
    assert!(got >= before && got <= after);
}

#[test]
fn ts_uses_provided_block_time() {
    let de = de_for(
        AnyEvent::Torch(TorchEvent::TokenRevived(
            torch_indexer::contracts::TokenRevived {
                mint: pk(0),
                total_contributed: 0,
                revival_slot: 0,
            },
        )),
        100,
        0,
    );
    assert_eq!(ts(&de), fixed_ts());
}

// ─── tier_from_target boundaries ────────────────────────────────────────

#[test]
fn tier_from_target_spark_at_10_sol() {
    const LAMPORTS: u64 = 1_000_000_000;
    assert_eq!(tier_from_target(LAMPORTS), MarketTier::Spark);
    assert_eq!(tier_from_target(10 * LAMPORTS), MarketTier::Spark);
}

#[test]
fn tier_from_target_flame_band() {
    const LAMPORTS: u64 = 1_000_000_000;
    assert_eq!(tier_from_target(11 * LAMPORTS), MarketTier::Flame);
    assert_eq!(tier_from_target(50 * LAMPORTS), MarketTier::Flame);
    assert_eq!(tier_from_target(100 * LAMPORTS), MarketTier::Flame);
}

#[test]
fn tier_from_target_torch_above_100() {
    const LAMPORTS: u64 = 1_000_000_000;
    assert_eq!(tier_from_target(101 * LAMPORTS), MarketTier::Torch);
    assert_eq!(tier_from_target(500 * LAMPORTS), MarketTier::Torch);
}

// ─── new_market ──────────────────────────────────────────────────────────

#[test]
fn new_market_empty_uri_becomes_none() {
    let event = MarketCreated {
        mint: pk(1),
        creator: pk(2),
        name: "n".into(),
        symbol: "n".into(),
        metadata_uri: String::new(),
        is_community_token: false,
        sol_target: 30_000_000_000,
        virtual_sol_reserves: 30_000_000_000,
        virtual_token_reserves: 1_073_000_000_000_000,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::MarketCreated(event.clone())),
        500,
        0,
    );
    let row = new_market(&event, &de);
    assert_eq!(row.metadata_uri, None);
    assert_eq!(row.status, MarketStatus::Rs);
    assert_eq!(row.tier, MarketTier::Flame); // 30 SOL target
    assert_eq!(row.created_at_slot, 500);
    assert_eq!(row.last_activity_slot, 500);
    assert_eq!(row.real_sol, 0); // freshly created
    assert_eq!(row.real_token, 0);
}

#[test]
fn new_market_non_empty_uri_preserved() {
    let event = MarketCreated {
        mint: pk(1),
        creator: pk(2),
        name: "n".into(),
        symbol: "n".into(),
        metadata_uri: "https://arweave.net/xyz".into(),
        is_community_token: true,
        sol_target: 100_000_000_000,
        virtual_sol_reserves: 0,
        virtual_token_reserves: 0,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::MarketCreated(event.clone())),
        1,
        0,
    );
    let row = new_market(&event, &de);
    assert_eq!(row.metadata_uri, Some("https://arweave.net/xyz".to_string()));
    assert!(row.is_community_token);
}

// ─── new_trade (vault sentinel) ─────────────────────────────────────────

#[test]
fn new_trade_default_pubkey_vault_becomes_none() {
    let event = BondingCurveTrade {
        mint: pk(1),
        trader: pk(2),
        vault: PUBKEY_DEFAULT,
        is_buy: true,
        sol_in: 1_000_000_000,
        sol_out: 0,
        tokens_in: 0,
        tokens_out: 500_000_000,
        sol_to_treasury: 5_000_000,
        sol_to_creator: 0,
        protocol_fee: 5_000_000,
        virtual_sol_after: 31_000_000_000,
        virtual_token_after: 1_073_000_000_000_000,
        real_sol_after: 1_000_000_000,
        real_token_after: 999_500_000_000,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::BondingCurveTrade(event.clone())),
        42,
        7,
    );
    let row = new_trade(&event, &de);
    assert_eq!(row.vault, None, "default-pubkey vault must map to None");
    assert!(row.is_buy);
    assert_eq!(row.sol_in, 1_000_000_000);
    assert_eq!(row.tokens_out, 500_000_000);
    assert_eq!(row.sol_out, 0);
    assert_eq!(row.tokens_in, 0);
    assert_eq!(row.slot, 42);
    assert_eq!(row.inner_ix_idx, 7);
    // Post-state mirroring.
    assert_eq!(row.virtual_sol_after, 31_000_000_000);
    assert_eq!(row.real_token_after, 999_500_000_000);
}

#[test]
fn new_trade_non_default_vault_preserved_as_b58() {
    let vault = pk_all(0xaa);
    let event = BondingCurveTrade {
        mint: pk(1),
        trader: pk(2),
        vault,
        is_buy: false,
        sol_in: 0,
        sol_out: 999_650_000,
        tokens_in: 1_000_000_000,
        tokens_out: 0,
        sol_to_treasury: 350_000,
        sol_to_creator: 0,
        protocol_fee: 0,
        virtual_sol_after: 0,
        virtual_token_after: 0,
        real_sol_after: 0,
        real_token_after: 0,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::BondingCurveTrade(event.clone())),
        1,
        0,
    );
    let row = new_trade(&event, &de);
    assert_eq!(row.vault, Some(b58(&vault)));
    assert!(!row.is_buy);
}

// ─── new_migration / pool_pubkey ────────────────────────────────────────

#[test]
fn new_migration_carries_all_fields() {
    let event = MigratedToDex {
        mint: pk(10),
        deep_pool: pk(11),
        sol_seeded: 30_000_000_000,
        tokens_seeded: 1_073_000_000_000_000,
        lp_burned: 28_274_333,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::MigratedToDex(event.clone())),
        9999,
        3,
    );
    let row = new_migration(&event, &de);
    assert_eq!(row.mint, b58(&pk(10)));
    assert_eq!(row.deep_pool_pubkey, b58(&pk(11)));
    assert_eq!(row.sol_seeded, 30_000_000_000);
    assert_eq!(row.tokens_seeded, 1_073_000_000_000_000);
    assert_eq!(row.lp_burned, 28_274_333);
    assert_eq!(row.slot, 9999);
}

#[test]
fn pool_pubkey_extracts_per_variant() {
    let p = PoolCreated {
        pool: pk(100),
        config: pk(0),
        token_mint: pk(0),
        lp_mint: pk(0),
        creator: pk(0),
        sol_in_gross: 0,
        sol_in_net: 0,
        tokens_in_gross: 0,
        tokens_in_net: 0,
        sol_reserve_after: 0,
        token_reserve_after: 0,
        lp_supply_after: 0,
        lp_to_creator: 0,
        lp_locked: 0,
    };
    assert_eq!(pool_pubkey(&DeepPoolEvent::PoolCreated(p.clone())), b58(&pk(100)));
}

// ─── new_pool ────────────────────────────────────────────────────────────

#[test]
fn new_pool_records_net_amounts() {
    let event = PoolCreated {
        pool: pk(200),
        config: pk(201),
        token_mint: pk(202),
        lp_mint: pk(203),
        creator: pk(204),
        sol_in_gross: 1_000_000_000,
        // net != gross under Token-2022 transfer fee. Translator should
        // record NET in sol_initial / tokens_initial (matches schema).
        sol_in_net: 999_300_000,
        tokens_in_gross: 1_000_000_000,
        tokens_in_net: 999_300_000,
        sol_reserve_after: 999_300_000,
        token_reserve_after: 999_300_000,
        lp_supply_after: 999_300_000,
        lp_to_creator: 999_300_000,
        lp_locked: 0,
    };
    let de = de_for(AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(event.clone())), 1, 0);
    let row = new_pool(&event, &de);
    assert_eq!(row.sol_initial, 999_300_000);
    assert_eq!(row.tokens_initial, 999_300_000);
    assert_eq!(row.lp_supply_initial, 999_300_000);
    assert_eq!(row.pubkey, b58(&pk(200)));
}

// ─── torch_event_mint dispatch ──────────────────────────────────────────

#[test]
fn torch_event_mint_returns_correct_mint_per_variant() {
    let m = pk(77);
    let s = OpenShortEvent {
        user: pk(0),
        mint: m,
        position_index: 0,
        collateral_sol_gross: 0,
        open_fee_sol: 0,
        net_collateral_sol: 0,
        tokens_borrowed: 0,
        vault_sol: 0,
    };
    assert_eq!(torch_event_mint(&TorchEvent::OpenShort(s)), b58(&m));

    let mc = MarketCreated {
        mint: m,
        creator: pk(0),
        name: "x".into(),
        symbol: "x".into(),
        metadata_uri: String::new(),
        is_community_token: false,
        sol_target: 0,
        virtual_sol_reserves: 0,
        virtual_token_reserves: 0,
    };
    assert_eq!(torch_event_mint(&TorchEvent::MarketCreated(mc)), b58(&m));
}

// ─── new_position_open_short net-recording semantics ────────────────────

#[test]
fn new_position_records_net_tokens_borrowed_unchanged() {
    // Event payload already carries the NET value (the program records net
    // post-fix). Translator must pass `tokens_borrowed` through as the position
    // `debt_amount` as-is — no further adjustment.
    let net_tokens = 999_300_000;
    let event = OpenShortEvent {
        user: pk(2),
        mint: pk(1),
        position_index: 0,
        collateral_sol_gross: 2_010_000_000,
        open_fee_sol: 10_000_000,
        net_collateral_sol: 2_000_000_000,
        tokens_borrowed: net_tokens,
        vault_sol: 2_000_000_000,
    };
    let de = de_for(
        AnyEvent::Torch(TorchEvent::OpenShort(event.clone())),
        100,
        0,
    );
    let row = new_position_open_short(&event, &de);
    assert_eq!(row.side, PositionSide::Short);
    assert_eq!(row.debt_amount, net_tokens as i64); // short debt = tokens
    assert_eq!(row.collateral_amount, 2_000_000_000); // short collateral = net SOL
    assert_eq!(row.open_fee_sol, 10_000_000);
    assert!(row.is_active);
    assert!(!row.owner_is_vault);
}
