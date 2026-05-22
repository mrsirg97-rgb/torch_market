// Event + row builders for integration tests.
//
// Each helper returns a sensible default; pass-through fields you care about
// at the call site. Keeps tests focused on the assertion, not the boilerplate.

#![allow(dead_code)]

use chrono::{DateTime, Utc};

use torch_indexer::contracts::{
    AnyEvent, BondingCurveTrade, DecodedEvent, DeepPoolEvent, MarketCreated, MarketStatus,
    MarketTier, MigratedToDex, NewLoanRow, NewMarketRow, NewPoolRow, NewShortRow, NewTradeRow,
    PoolCreated, PositionHealth, ShortOpened, SwapExecuted, TorchEvent,
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
    }
}

pub fn de_with_memo(event: AnyEvent, slot: i64, inner_ix_idx: i32, memo: &str) -> DecodedEvent {
    let mut e = de(event, slot, inner_ix_idx);
    e.memo = Some(memo.to_string());
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

pub fn short_opened(mint: u8, user: u8, net_tokens: u64) -> ShortOpened {
    ShortOpened {
        mint: pk(mint),
        user: pk(user),
        sol_collateral: 2_000_000_000,
        tokens_borrowed: net_tokens, // already net (post-fix semantics)
        ltv_bps: 4500,
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

pub fn new_loan_row(mint: &str, borrower: &str, slot: i64) -> NewLoanRow {
    NewLoanRow {
        mint: mint.to_string(),
        borrower: borrower.to_string(),
        collateral_amount: 100_000_000_000,
        borrowed_amount: 2_000_000_000,
        accrued_interest_stored: 0,
        last_update_slot: slot,
        health: PositionHealth::Healthy,
        is_active: true,
        created_at: fixed_ts(),
        updated_at: fixed_ts(),
    }
}

pub fn new_short_row(mint: &str, shorter: &str, slot: i64) -> NewShortRow {
    NewShortRow {
        mint: mint.to_string(),
        shorter: shorter.to_string(),
        sol_collateral: 2_000_000_000,
        tokens_borrowed: 999_300_000, // net post-fee
        accrued_interest_stored: 0,
        last_update_slot: slot,
        health: PositionHealth::Healthy,
        is_active: true,
        created_at: fixed_ts(),
        updated_at: fixed_ts(),
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

pub fn ev_short_opened(mint: u8, user: u8, net_tokens: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::ShortOpened(short_opened(mint, user, net_tokens)))
}
