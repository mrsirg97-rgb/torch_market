// Translation layer: decoded events → DB insert shapes.
//
// Pure mapping, no IO. Lives between the decoder (event payloads) and the
// writer (per-table batches). Pubkey [u8; 32] arrays become base58 strings.
// u64 → i64 cast is safe at realistic SOL/token magnitudes (i64 holds up to
// 9.2e18; max Solana lamports ≈ 4.3e15; TOTAL_SUPPLY ≈ 1e15 raw).

use chrono::{DateTime, Utc};

use crate::contracts::{
    BondingCurveTrade, CloseLongEvent, CloseShortEvent, DecodedEvent, DeepPoolEvent,
    LiquidateLongEvent, LiquidateShortEvent, LiquidityAdded, LiquidityRemoved, MarketCreated,
    MarketStatus, MarketTier, MigratedToDex, NewLiquidityRow, NewMarketRow, NewMessageRow,
    NewMigrationRow, NewPoolRow, NewPositionEventRow, NewPositionRow, NewReservesRow, NewSwapRow,
    NewTradeRow, OpenLongEvent, OpenShortEvent, PoolCreated, PositionEventKind, PositionHealth,
    PositionSide, SwapExecuted, TorchEvent,
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
    // Spark (50 SOL) removed from the program; anything ≤ 100 SOL is flame.
    if sol_target <= 100 * LAMPORTS_PER_SOL {
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
// torch — markets / trades / messages / positions / migrations
// ============================================================================

// Mint of the torch event, base58. Pulled out because the writer needs it
// before deciding which table mutation to perform.
pub fn torch_event_mint(event: &TorchEvent) -> String {
    match event {
        TorchEvent::MarketCreated(e) => b58(&e.mint),
        TorchEvent::BondingCurveTrade(e) => b58(&e.mint),
        TorchEvent::MigratedToDex(e) => b58(&e.mint),
        TorchEvent::VaultSwapExecuted(e) => b58(&e.mint),
        TorchEvent::OpenShort(e) => b58(&e.mint),
        TorchEvent::CloseShort(e) => b58(&e.mint),
        TorchEvent::LiquidateShort(e) => b58(&e.mint),
        TorchEvent::OpenLong(e) => b58(&e.mint),
        TorchEvent::CloseLong(e) => b58(&e.mint),
        TorchEvent::LiquidateLong(e) => b58(&e.mint),
        TorchEvent::BondingCompleted(e) => b58(&e.mint),
        TorchEvent::TokenReclaimed(e) => b58(&e.mint),
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
        status: MarketStatus::Bonding,
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

// [V21] Leverage positions — unified current snapshot per
// (mint, owner, side, position_index). Open events carry absolute amounts so
// they map directly to a fresh `positions` upsert; close/liquidate carry deltas
// and need the prior row (see stream/writer.rs). `health` is a coarse snapshot
// here; the API recomputes live health against the deep_pool TWAP at read time.
//
// Unit-by-side (mirrors the on-chain generic fields):
//   short → collateral = lamports (SOL), debt = tokens, vault_balance = lamports
//   long  → collateral = tokens,         debt = SOL,    vault_balance = tokens

// `via_vault` ⇒ owner is a TorchVault PDA (the event's `user`/`borrower` is
// already the vault key for those variants).
#[allow(clippy::too_many_arguments)]
fn open_position_base(
    mint: &str,
    owner: &str,
    side: PositionSide,
    position_index: u32,
    collateral_amount: u64,
    debt_amount: u64,
    open_fee_sol: u64,
    vault_balance: u64,
    via_vault: bool,
    de: &DecodedEvent,
) -> NewPositionRow {
    let now = ts(de);
    NewPositionRow {
        mint: mint.to_string(),
        owner: owner.to_string(),
        side,
        position_index: position_index as i32,
        collateral_amount: collateral_amount as i64,
        debt_amount: debt_amount as i64,
        open_fee_sol: open_fee_sol as i64,
        vault_balance: vault_balance as i64,
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        health: PositionHealth::Healthy,
        is_active: true,
        owner_is_vault: via_vault,
        created_at: now,
        updated_at: now,
    }
}

pub fn new_position_open_short(e: &OpenShortEvent, de: &DecodedEvent) -> NewPositionRow {
    open_position_base(
        &b58(&e.mint),
        &b58(&e.user),
        PositionSide::Short,
        e.position_index,
        e.net_collateral_sol,
        e.tokens_borrowed,
        e.open_fee_sol,
        e.vault_sol,
        de.via_vault,
        de,
    )
}

pub fn new_position_open_long(e: &OpenLongEvent, de: &DecodedEvent) -> NewPositionRow {
    open_position_base(
        &b58(&e.mint),
        &b58(&e.user),
        PositionSide::Long,
        e.position_index,
        e.collateral_tokens,
        e.borrowed_sol_gross,
        e.open_fee_sol,
        e.vault_tokens,
        de.via_vault,
        de,
    )
}

// ── position_events log builders (pure; append-only, idempotent on sig+idx) ──

#[allow(clippy::too_many_arguments)]
fn pos_event_base(
    mint: &str,
    owner: &str,
    side: PositionSide,
    position_index: u32,
    kind: PositionEventKind,
    de: &DecodedEvent,
) -> NewPositionEventRow {
    NewPositionEventRow {
        mint: mint.to_string(),
        owner: owner.to_string(),
        side,
        position_index: position_index as i32,
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
        slot: de.slot,
        signature: de.signature.clone(),
        inner_ix_idx: de.inner_ix_idx,
        created_at: ts(de),
    }
}

pub fn pos_event_open_short(e: &OpenShortEvent, de: &DecodedEvent) -> NewPositionEventRow {
    NewPositionEventRow {
        sol_in: Some(e.collateral_sol_gross as i64),
        tokens_out: Some(e.tokens_borrowed as i64),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.user),
            PositionSide::Short,
            e.position_index,
            PositionEventKind::Open,
            de,
        )
    }
}

pub fn pos_event_open_long(e: &OpenLongEvent, de: &DecodedEvent) -> NewPositionEventRow {
    NewPositionEventRow {
        tokens_in: Some(e.collateral_tokens as i64),
        sol_out: Some(e.atomic_buy_sol as i64),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.user),
            PositionSide::Long,
            e.position_index,
            PositionEventKind::Open,
            de,
        )
    }
}

pub fn pos_event_close_short(e: &CloseShortEvent, de: &DecodedEvent) -> NewPositionEventRow {
    NewPositionEventRow {
        tokens_in: Some(e.debt_repaid as i64),
        sol_in: Some(e.sol_spent_on_buyback as i64),
        sol_out: Some(e.surplus_sol_to_user as i64),
        interest_paid: Some(e.interest_paid as i64),
        principal_paid: Some(e.principal_paid as i64),
        surplus_sol: Some(e.surplus_sol_to_user as i64),
        fully_resolved: Some(e.fully_closed),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.user),
            PositionSide::Short,
            e.position_index,
            PositionEventKind::Close,
            de,
        )
    }
}

pub fn pos_event_close_long(e: &CloseLongEvent, de: &DecodedEvent) -> NewPositionEventRow {
    NewPositionEventRow {
        tokens_out: Some(e.tokens_sold as i64),
        sol_out: Some(e.sol_out as i64),
        interest_paid: Some(e.interest_paid as i64),
        principal_paid: Some(e.principal_paid as i64),
        surplus_sol: Some(e.surplus_sol_to_user as i64),
        fully_resolved: Some(e.fully_closed),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.user),
            PositionSide::Long,
            e.position_index,
            PositionEventKind::Close,
            de,
        )
    }
}

pub fn pos_event_liquidate_short(
    e: &LiquidateShortEvent,
    de: &DecodedEvent,
) -> NewPositionEventRow {
    NewPositionEventRow {
        liquidator: Some(b58(&e.liquidator)),
        tokens_in: Some(e.tokens_covered as i64),
        bad_debt: Some(e.bad_debt as i64),
        twap_ltv: Some(e.twap_ltv as i64),
        bonus_bps: Some(e.bonus_bps as i32),
        seized: Some(e.sol_seized as i64),
        residual: Some(e.residual_sol_to_borrower as i64),
        fully_resolved: Some(e.fully_liquidated),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.borrower),
            PositionSide::Short,
            e.position_index,
            PositionEventKind::Liquidate,
            de,
        )
    }
}

pub fn pos_event_liquidate_long(e: &LiquidateLongEvent, de: &DecodedEvent) -> NewPositionEventRow {
    NewPositionEventRow {
        liquidator: Some(b58(&e.liquidator)),
        sol_in: Some(e.debt_covered as i64),
        bad_debt: Some(e.bad_debt as i64),
        twap_ltv: Some(e.twap_ltv as i64),
        bonus_bps: Some(e.bonus_bps as i32),
        seized: Some(e.tokens_seized as i64),
        fully_resolved: Some(e.fully_liquidated),
        ..pos_event_base(
            &b58(&e.mint),
            &b58(&e.borrower),
            PositionSide::Long,
            e.position_index,
            PositionEventKind::Liquidate,
            de,
        )
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

// Close*/Liquidate* events carry deltas (debt_repaid, tokens_covered, …), not
// absolute amounts. The "apply delta to the existing position row" mutation is
// expressed at the writer layer (which has DB access to read the prior row)
// rather than as a standalone translate function. The position_events log rows
// above ARE pure and built here; only the `positions` current-state reconcile
// for close/liquidate lives in stream/writer.rs.
