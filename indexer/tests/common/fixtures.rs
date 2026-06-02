// Event + row builders for integration tests.
//
// Each helper returns a sensible default; pass-through fields you care about
// at the call site. Keeps tests focused on the assertion, not the boilerplate.

#![allow(dead_code)]

use chrono::{DateTime, Utc};

use torch_indexer::contracts::{
    AnyEvent, BondingCurveTrade, CloseShortEvent, DecodedEvent, DeepPoolEvent, LiquidateShortEvent,
    MarketCreated, MarketStatus, MarketTier, MigratedToDex, NewMarketRow, NewPoolRow,
    NewPositionEventRow, NewPositionRow, NewTradeRow, OpenLongEvent, OpenShortEvent, PoolCreated,
    PositionEventKind, PositionHealth, PositionSide, SwapExecuted, TorchEvent,
};

// ─── pubkey helpers ──────────────────────────────────────────────────────

pub fn pk(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = byte;
    out
}

pub fn pk58(byte: u8) -> String {
    bs58::encode(pk(byte)).into_string()
}

pub fn fixed_ts() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

// ─── DecodedEvent wrappers ───────────────────────────────────────────────

pub fn de(event: AnyEvent, slot: i64, inner_ix_idx: i32) -> DecodedEvent {
    DecodedEvent {
        signature: format!("sig_{slot}_{inner_ix_idx}"),
        inner_ix_idx,
        slot,
        block_time: Some(fixed_ts()),
        event,
        memo: None,
        via_vault: false,
    }
}

pub fn de_with_memo(event: AnyEvent, slot: i64, inner_ix_idx: i32, memo: &str) -> DecodedEvent {
    let mut e = de(event, slot, inner_ix_idx);
    e.memo = Some(memo.to_string());
    e
}

// [V21] Same as `de` but flags the emitting ix as a `*_via_vault` variant
// (owner_is_vault = true on the resulting position).
pub fn de_via_vault(event: AnyEvent, slot: i64, inner_ix_idx: i32) -> DecodedEvent {
    let mut e = de(event, slot, inner_ix_idx);
    e.via_vault = true;
    e
}

// ─── on-chain event payload builders ─────────────────────────────────────

pub fn market_created(mint: u8, creator: u8) -> MarketCreated {
    MarketCreated {
        mint: pk(mint),
        creator: pk(creator),
        name: "TestMint".to_string(),
        symbol: "TST".to_string(),
        metadata_uri: "https://arweave.net/test".to_string(),
        is_community_token: false,
        sol_target: 30_000_000_000, // 30 SOL → Flame tier
        virtual_sol_reserves: 30_000_000_000,
        virtual_token_reserves: 1_073_000_000_000_000,
    }
}

pub fn buy_trade(mint: u8, trader: u8, sol_in: u64, tokens_out: u64) -> BondingCurveTrade {
    // Reserves-after fields use saturating_sub so callers can pass arbitrary
    // tokens_out without worrying about underflowing the synthetic starting
    // reserves. The exact post-state value isn't asserted on in any test —
    // it's only there for the writer's `apply_trade` UPDATE to land valid data.
    BondingCurveTrade {
        mint: pk(mint),
        trader: pk(trader),
        vault: [0u8; 32],
        is_buy: true,
        sol_in,
        sol_out: 0,
        tokens_in: 0,
        tokens_out,
        sol_to_treasury: sol_in / 200, // 0.5% fee
        sol_to_creator: 0,
        protocol_fee: sol_in / 200,
        virtual_sol_after: 30_000_000_000u64.saturating_add(sol_in),
        virtual_token_after: 1_073_000_000_000_000u64.saturating_sub(tokens_out),
        real_sol_after: sol_in,
        real_token_after: 800_000_000_000_000u64.saturating_sub(tokens_out),
    }
}

pub fn pool_created(pool: u8, mint: u8, creator: u8) -> PoolCreated {
    PoolCreated {
        pool: pk(pool),
        config: pk(99),
        token_mint: pk(mint),
        lp_mint: pk(98),
        creator: pk(creator),
        sol_in_gross: 30_000_000_000,
        sol_in_net: 30_000_000_000,
        tokens_in_gross: 1_073_000_000_000_000,
        tokens_in_net: 1_073_000_000_000_000,
        sol_reserve_after: 30_000_000_000,
        token_reserve_after: 1_073_000_000_000_000,
        lp_supply_after: 28_274_333,
        lp_to_creator: 28_274_333,
        lp_locked: 0,
    }
}

pub fn migrated_to_dex(mint: u8, pool: u8) -> MigratedToDex {
    MigratedToDex {
        mint: pk(mint),
        deep_pool: pk(pool),
        sol_seeded: 30_000_000_000,
        tokens_seeded: 1_073_000_000_000_000,
        lp_burned: 28_274_333,
    }
}

pub fn swap_executed(pool: u8, user: u8, is_buy: bool) -> SwapExecuted {
    SwapExecuted {
        pool: pk(pool),
        user: pk(user),
        sol_source: pk(user),
        buy: is_buy,
        amount_in_gross: 500_000_000,
        amount_in_net: 499_650_000,
        amount_out_gross: 1_234_567,
        amount_out_net: 1_233_703,
        fee: 350_000,
        sol_reserve_after: 30_500_000_000,
        token_reserve_after: 1_072_998_765_433,
    }
}

// [V21] OpenShortEvent — `tokens_borrowed` is the net (post-Token-2022-fee)
// amount, matching the on-chain net-recording semantics.
pub fn open_short_event(mint: u8, user: u8, net_tokens: u64) -> OpenShortEvent {
    OpenShortEvent {
        user: pk(user),
        mint: pk(mint),
        position_index: 0,
        collateral_sol_gross: 2_010_000_000,
        open_fee_sol: 10_000_000,
        net_collateral_sol: 2_000_000_000,
        tokens_borrowed: net_tokens,
        vault_sol: 2_000_000_000,
    }
}

pub fn open_long_event(mint: u8, user: u8, collateral_tokens: u64) -> OpenLongEvent {
    OpenLongEvent {
        user: pk(user),
        mint: pk(mint),
        position_index: 0,
        collateral_tokens,
        borrowed_sol_gross: 1_000_000_000,
        open_fee_sol: 5_000_000,
        atomic_buy_sol: 1_000_000_000,
        vault_tokens: collateral_tokens,
    }
}

pub fn close_short_event(mint: u8, user: u8, debt_repaid: u64, fully_closed: bool) -> CloseShortEvent {
    CloseShortEvent {
        user: pk(user),
        mint: pk(mint),
        position_index: 0,
        debt_repaid,
        sol_spent_on_buyback: 1_500_000_000,
        interest_paid: 20_000_000,
        principal_paid: 1_480_000_000,
        surplus_sol_to_user: 480_000_000,
        fully_closed,
    }
}

pub fn liquidate_short_event(
    mint: u8,
    liquidator: u8,
    borrower: u8,
    tokens_covered: u64,
    fully_liquidated: bool,
) -> LiquidateShortEvent {
    LiquidateShortEvent {
        liquidator: pk(liquidator),
        borrower: pk(borrower),
        mint: pk(mint),
        position_index: 0,
        tokens_covered,
        sol_seized: 1_900_000_000,
        bad_debt: 0,
        bonus_bps: 500,
        twap_ltv: 9200,
        residual_sol_to_borrower: 50_000_000,
        fully_liquidated,
    }
}

// ─── DB insert-row shapes (when bypassing the event/translate layer) ────

pub fn new_market_row(mint: &str, creator: &str) -> NewMarketRow {
    NewMarketRow {
        mint: mint.to_string(),
        name: "TestMint".to_string(),
        symbol: "TST".to_string(),
        metadata_uri: Some("https://arweave.net/test".to_string()),
        creator: creator.to_string(),
        is_community_token: false,
        status: MarketStatus::Rs,
        tier: MarketTier::Flame,
        sol_target: 30_000_000_000,
        virtual_sol: 30_000_000_000,
        virtual_token: 1_073_000_000_000_000,
        real_sol: 0,
        real_token: 0,
        created_at_slot: 100,
        last_activity_slot: 100,
        created_at: fixed_ts(),
    }
}

pub fn new_pool_row(pubkey: &str, mint: &str, creator: &str) -> NewPoolRow {
    NewPoolRow {
        pubkey: pubkey.to_string(),
        config: pk58(99),
        token_mint: mint.to_string(),
        lp_mint: pk58(98),
        creator: creator.to_string(),
        sol_initial: 30_000_000_000,
        tokens_initial: 1_073_000_000_000_000,
        lp_supply_initial: 28_274_333,
        slot: 100,
        signature: "sig_pool_create".to_string(),
        created_at: fixed_ts(),
    }
}

pub fn new_trade_row(mint: &str, trader: &str, slot: i64, inner: i32) -> NewTradeRow {
    NewTradeRow {
        mint: mint.to_string(),
        trader: trader.to_string(),
        vault: None,
        is_buy: true,
        sol_in: 1_000_000_000,
        sol_out: 0,
        tokens_in: 0,
        tokens_out: 315_000_000_000_000,
        sol_to_treasury: 5_000_000,
        sol_to_creator: 0,
        protocol_fee: 5_000_000,
        virtual_sol_after: 31_000_000_000,
        virtual_token_after: 1_072_685_000_000_000,
        real_sol_after: 1_000_000_000,
        real_token_after: 800_000_000_000,
        slot,
        signature: format!("sig_trade_{slot}_{inner}"),
        inner_ix_idx: inner,
        created_at: fixed_ts(),
    }
}

// [V21] Unified position row. `side` selects the unit-by-side defaults:
//   short → collateral = SOL (2 SOL), debt = tokens (net)
//   long  → collateral = tokens,      debt = SOL
pub fn new_position_row(
    mint: &str,
    owner: &str,
    side: PositionSide,
    position_index: i32,
    slot: i64,
) -> NewPositionRow {
    let (collateral_amount, debt_amount, vault_balance) = match side {
        PositionSide::Short => (2_000_000_000, 999_300_000, 2_000_000_000),
        PositionSide::Long => (315_000_000_000_000, 1_000_000_000, 315_000_000_000_000),
    };
    NewPositionRow {
        mint: mint.to_string(),
        owner: owner.to_string(),
        side,
        position_index,
        collateral_amount,
        debt_amount,
        open_fee_sol: 10_000_000,
        vault_balance,
        accrued_interest_stored: 0,
        last_update_slot: slot,
        health: PositionHealth::Healthy,
        is_active: true,
        owner_is_vault: false,
        created_at: fixed_ts(),
        updated_at: fixed_ts(),
    }
}

// [V21] A position_events log row (append-only). Defaults to an `open` event;
// override `kind` + the kind-specific fields at the call site.
pub fn new_position_event_row(
    mint: &str,
    owner: &str,
    side: PositionSide,
    kind: PositionEventKind,
    slot: i64,
    inner: i32,
) -> NewPositionEventRow {
    NewPositionEventRow {
        mint: mint.to_string(),
        owner: owner.to_string(),
        side,
        position_index: 0,
        kind,
        liquidator: None,
        sol_in: None,
        sol_out: None,
        tokens_in: None,
        tokens_out: None,
        interest_paid: None,
        principal_paid: None,
        surplus_sol: None,
        bad_debt: None,
        twap_ltv: None,
        bonus_bps: None,
        seized: None,
        residual: None,
        fully_resolved: None,
        slot,
        signature: format!("sig_posevt_{slot}_{inner}"),
        inner_ix_idx: inner,
        created_at: fixed_ts(),
    }
}

// AnyEvent wrappers for the writer.
pub fn ev_market_created(mint: u8, creator: u8) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::MarketCreated(market_created(mint, creator)))
}

pub fn ev_buy_trade(mint: u8, trader: u8, sol_in: u64, tokens_out: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::BondingCurveTrade(buy_trade(
        mint, trader, sol_in, tokens_out,
    )))
}

pub fn ev_pool_created(pool: u8, mint: u8, creator: u8) -> AnyEvent {
    AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(pool_created(pool, mint, creator)))
}

pub fn ev_migrated(mint: u8, pool: u8) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::MigratedToDex(migrated_to_dex(mint, pool)))
}

pub fn ev_swap(pool: u8, user: u8, is_buy: bool) -> AnyEvent {
    AnyEvent::DeepPool(DeepPoolEvent::SwapExecuted(swap_executed(pool, user, is_buy)))
}

pub fn ev_open_short(mint: u8, user: u8, net_tokens: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::OpenShort(open_short_event(mint, user, net_tokens)))
}

pub fn ev_open_long(mint: u8, user: u8, collateral_tokens: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::OpenLong(open_long_event(
        mint,
        user,
        collateral_tokens,
    )))
}

pub fn ev_close_short(mint: u8, user: u8, debt_repaid: u64, fully_closed: bool) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::CloseShort(close_short_event(
        mint,
        user,
        debt_repaid,
        fully_closed,
    )))
}

pub fn ev_liquidate_short(
    mint: u8,
    liquidator: u8,
    borrower: u8,
    tokens_covered: u64,
    fully_liquidated: bool,
) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::LiquidateShort(liquidate_short_event(
        mint,
        liquidator,
        borrower,
        tokens_covered,
        fully_liquidated,
    )))
}
