// Translation layer: decoded events → DB insert shapes.
//
// Pure mapping, no IO. Lives between the decoder (event payloads) and the
// writer (per-table batches). Pubkey [u8; 32] arrays become base58 strings.
// u64 → i64 cast is safe at realistic SOL/token magnitudes (i64 holds up to
// 9.2e18; max Solana lamports ≈ 4.3e15; TOTAL_SUPPLY ≈ 1e15 raw).

use chrono::{DateTime, Utc};

use crate::contracts::{
    BondingCurveTrade, DecodedEvent, DeepPoolEvent, LiquidityAdded, LiquidityRemoved, LoanCreated,
    MarketCreated, MarketStatus, MarketTier, MigratedToDex, NewLiquidityRow, NewLoanRow,
    NewMarketRow, NewMessageRow, NewMigrationRow, NewPoolRow, NewReservesRow, NewShortRow,
    NewSwapRow, NewTradeRow, PoolCreated, PositionHealth, ShortOpened, SwapExecuted, TorchEvent,
};

// ============================================================================
// Helpers
// ============================================================================

pub fn b58(bytes: &[u8; 32]) -> String {
    bs58::encode(bytes).into_string()
}

pub fn ts(de: &DecodedEvent) -> DateTime<Utc> {
    de.block_time.unwrap_or_else(Utc::now)
}

// Pubkey::default() bytes — used as the "no vault" sentinel in BondingCurveTrade.
pub const PUBKEY_DEFAULT: [u8; 32] = [0u8; 32];

// Map sol_target → market tier. Mirrors VALID_BONDING_TARGETS in the on-chain
// constants. spark = small, flame = medium, torch = max.
pub fn tier_from_target(sol_target: u64) -> MarketTier {
    // Conservative bucketing: any target ≤ 10 SOL = spark, ≤ 100 = flame,
    // otherwise torch. Matches the program's BONDING_TARGET_SPARK/FLAME/TORCH
    // constants without needing to import them.
    const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
    if sol_target <= 10 * LAMPORTS_PER_SOL {
        MarketTier::Spark
    } else if sol_target <= 100 * LAMPORTS_PER_SOL {
        MarketTier::Flame
    } else {
        MarketTier::Torch
    }
}

// ============================================================================
// deep_pool — pool/swap/liquidity/reserves translators
// ============================================================================

pub fn pool_pubkey(event: &DeepPoolEvent) -> String {
    b58(pool_bytes(event))
}

fn pool_bytes(event: &DeepPoolEvent) -> &[u8; 32] {
    match event {
        DeepPoolEvent::PoolCreated(p) => &p.pool,
        DeepPoolEvent::SwapExecuted(s) => &s.pool,
        DeepPoolEvent::LiquidityAdded(la) => &la.pool,
        DeepPoolEvent::LiquidityRemoved(lr) => &lr.pool,
    }
}

pub fn new_pool(p: &PoolCreated, de: &DecodedEvent) -> NewPoolRow {
    NewPoolRow {
        pubkey: b58(&p.pool),
        config: b58(&p.config),
        token_mint: b58(&p.token_mint),
        lp_mint: b58(&p.lp_mint),
        creator: b58(&p.creator),
        sol_initial: p.sol_in_net as i64,
        tokens_initial: p.tokens_in_net as i64,
        lp_supply_initial: p.lp_supply_after as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        created_at: ts(de),
    }
}

pub fn new_swap(s: &SwapExecuted, pool_id: i32, de: &DecodedEvent) -> NewSwapRow {
    NewSwapRow {
        pool_id,
        user_pk: b58(&s.user),
        sol_source: b58(&s.sol_source),
        is_buy: s.buy,
        amount_in_gross: s.amount_in_gross as i64,
        amount_in_net: s.amount_in_net as i64,
        amount_out_gross: s.amount_out_gross as i64,
        amount_out_net: s.amount_out_net as i64,
        fee: s.fee as i64,
        sol_reserve_after: s.sol_reserve_after as i64,
        token_reserve_after: s.token_reserve_after as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

pub fn new_liquidity_add(la: &LiquidityAdded, pool_id: i32, de: &DecodedEvent) -> NewLiquidityRow {
    NewLiquidityRow {
        pool_id,
        provider: b58(&la.provider),
        is_add: true,
        sol_amount_gross: la.sol_in_gross as i64,
        sol_amount_net: la.sol_in_net as i64,
        tokens_amount_gross: la.tokens_in_gross as i64,
        tokens_amount_net: la.tokens_in_net as i64,
        lp_user_amount: la.lp_to_provider as i64,
        lp_locked: la.lp_locked as i64,
        lp_supply_after: la.lp_supply_after as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

pub fn new_liquidity_remove(
    lr: &LiquidityRemoved,
    pool_id: i32,
    de: &DecodedEvent,
) -> NewLiquidityRow {
    NewLiquidityRow {
        pool_id,
        provider: b58(&lr.provider),
        is_add: false,
        sol_amount_gross: lr.sol_out_gross as i64,
        sol_amount_net: lr.sol_out_net as i64,
        tokens_amount_gross: lr.tokens_out_gross as i64,
        tokens_amount_net: lr.tokens_out_net as i64,
        lp_user_amount: lr.lp_burned as i64,
        lp_locked: 0,
        lp_supply_after: lr.lp_supply_after as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

pub fn new_reserves(
    pool_id: i32,
    sol_reserve: i64,
    token_reserve: i64,
    lp_supply: i64,
    de: &DecodedEvent,
) -> NewReservesRow {
    NewReservesRow {
        pool_id,
        sol_reserve,
        token_reserve,
        lp_supply,
        last_slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

// ============================================================================
// torch — markets / trades / messages / loans / shorts / migrations
// ============================================================================

// Mint of the torch event, base58. Pulled out because the writer needs it
// before deciding which table mutation to perform.
pub fn torch_event_mint(event: &TorchEvent) -> String {
    match event {
        TorchEvent::MarketCreated(e) => b58(&e.mint),
        TorchEvent::BondingCurveTrade(e) => b58(&e.mint),
        TorchEvent::MigratedToDex(e) => b58(&e.mint),
        TorchEvent::VaultSwapExecuted(e) => b58(&e.mint),
        TorchEvent::LoanCreated(e) => b58(&e.mint),
        TorchEvent::LoanRepaid(e) => b58(&e.mint),
        TorchEvent::LoanLiquidated(e) => b58(&e.mint),
        TorchEvent::ShortOpened(e) => b58(&e.mint),
        TorchEvent::ShortClosed(e) => b58(&e.mint),
        TorchEvent::ShortLiquidated(e) => b58(&e.mint),
        TorchEvent::RevivalContribution(e) => b58(&e.mint),
        TorchEvent::TokenRevived(e) => b58(&e.mint),
    }
}

pub fn new_market(m: &MarketCreated, de: &DecodedEvent) -> NewMarketRow {
    NewMarketRow {
        mint: b58(&m.mint),
        name: m.name.clone(),
        symbol: m.symbol.clone(),
        metadata_uri: if m.metadata_uri.is_empty() {
            None
        } else {
            Some(m.metadata_uri.clone())
        },
        creator: b58(&m.creator),
        is_community_token: m.is_community_token,
        status: MarketStatus::Rs,
        tier: tier_from_target(m.sol_target),
        sol_target: m.sol_target as i64,
        virtual_sol: m.virtual_sol_reserves as i64,
        virtual_token: m.virtual_token_reserves as i64,
        real_sol: 0,
        real_token: 0,
        created_at_slot: de.slot,
        last_activity_slot: de.slot,
        created_at: ts(de),
    }
}

pub fn new_trade(t: &BondingCurveTrade, de: &DecodedEvent) -> NewTradeRow {
    NewTradeRow {
        mint: b58(&t.mint),
        trader: b58(&t.trader),
        vault: if t.vault == PUBKEY_DEFAULT {
            None
        } else {
            Some(b58(&t.vault))
        },
        is_buy: t.is_buy,
        sol_in: t.sol_in as i64,
        sol_out: t.sol_out as i64,
        tokens_in: t.tokens_in as i64,
        tokens_out: t.tokens_out as i64,
        sol_to_treasury: t.sol_to_treasury as i64,
        sol_to_creator: t.sol_to_creator as i64,
        protocol_fee: t.protocol_fee as i64,
        virtual_sol_after: t.virtual_sol_after as i64,
        virtual_token_after: t.virtual_token_after as i64,
        real_sol_after: t.real_sol_after as i64,
        real_token_after: t.real_token_after as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

pub fn new_migration(m: &MigratedToDex, de: &DecodedEvent) -> NewMigrationRow {
    NewMigrationRow {
        mint: b58(&m.mint),
        deep_pool_pubkey: b58(&m.deep_pool),
        sol_seeded: m.sol_seeded as i64,
        tokens_seeded: m.tokens_seeded as i64,
        lp_burned: m.lp_burned as i64,
        slot: de.slot,
        signature: de.signature.clone(),
        created_at: ts(de),
    }
}

// Lending — current snapshot per (mint, borrower). LoanCreated → upsert with
// is_active=true; LoanRepaid (fully) → upsert is_active=false; LoanLiquidated
// → upsert is_active=false. health is left as Healthy at write time; service
// layer recomputes at read time using the projected interest from
// last_update_slot.
pub fn new_loan_from_created(e: &LoanCreated, de: &DecodedEvent) -> NewLoanRow {
    let now = ts(de);
    NewLoanRow {
        mint: b58(&e.mint),
        borrower: b58(&e.user),
        collateral_amount: e.collateral_amount as i64,
        borrowed_amount: e.borrowed_amount as i64,
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        health: PositionHealth::Healthy,
        is_active: true,
        created_at: now,
        updated_at: now,
    }
}

// LoanRepaid carries deltas, not absolute amounts. Writer needs to look up
// the existing row to apply the diff — translate just returns a partial
// "post-state" stub the writer will reconcile against the DB. For now we
// surface the event payload as-is and let the writer compute.
//
// To keep the translate layer pure, the writer reads the row, subtracts
// `sol_repaid`/`interest_paid`/`collateral_returned`, then upserts.
// LoanLiquidated similar shape.

// Shorts — same pattern.
pub fn new_short_from_opened(e: &ShortOpened, de: &DecodedEvent) -> NewShortRow {
    let now = ts(de);
    NewShortRow {
        mint: b58(&e.mint),
        shorter: b58(&e.user),
        sol_collateral: e.sol_collateral as i64,
        tokens_borrowed: e.tokens_borrowed as i64,
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        health: PositionHealth::Healthy,
        is_active: true,
        created_at: now,
        updated_at: now,
    }
}

// Memo attribution. The writer calls this with a pre-decoded memo text
// (extracted from the same tx's memo ix) when persisting a trade.
pub fn new_message(
    mint: &str,
    sender: [u8; 32],
    memo_text: String,
    action_kind: Option<&str>,
    de: &DecodedEvent,
) -> NewMessageRow {
    NewMessageRow {
        mint: mint.to_string(),
        sender: b58(&sender),
        memo_text,
        action_kind: action_kind.map(|s| s.to_string()),
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

// LoanRepaid / LoanLiquidated / ShortClosed / ShortLiquidated produce
// "apply delta to existing row" mutations that are easier to express at the
// writer layer (which has DB access to read the prior row) than as
// standalone translate functions. See stream/writer.rs.
