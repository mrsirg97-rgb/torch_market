// Type contracts that cross module boundaries. Three things live here:
//   1. Borsh shapes for on-chain event payloads (one per #[event] in each
//      program). Field order MUST mirror the program — Borsh is positional.
//   2. DB row types (sqlx::FromRow) and New*Row insert shapes, mirroring
//      indexer/db/01-schema.sql.
//   3. AppState + Broadcaster wiring shared by the writer (publish) and
//      the WS handler (subscribe).

use std::sync::Arc;

use borsh::BorshDeserialize;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::constants::BROADCAST_CAPACITY;

// ============================================================================
// On-chain event Borsh shapes — deep_pool
// ============================================================================
//
// Borsh field ORDER mirrors programs/deep_pool/src/events.rs exactly.

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct PoolCreated {
    pub pool: [u8; 32],
    pub config: [u8; 32],
    pub token_mint: [u8; 32],
    pub lp_mint: [u8; 32],
    pub creator: [u8; 32],
    pub sol_in_gross: u64,
    pub sol_in_net: u64,
    pub tokens_in_gross: u64,
    pub tokens_in_net: u64,
    pub sol_reserve_after: u64,
    pub token_reserve_after: u64,
    pub lp_supply_after: u64,
    pub lp_to_creator: u64,
    pub lp_locked: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct LiquidityAdded {
    pub pool: [u8; 32],
    pub provider: [u8; 32],
    pub sol_in_gross: u64,
    pub sol_in_net: u64,
    pub tokens_in_gross: u64,
    pub tokens_in_net: u64,
    pub lp_to_provider: u64,
    pub lp_locked: u64,
    pub sol_reserve_after: u64,
    pub token_reserve_after: u64,
    pub lp_supply_after: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct LiquidityRemoved {
    pub pool: [u8; 32],
    pub provider: [u8; 32],
    pub lp_burned: u64,
    pub sol_out_gross: u64,
    pub sol_out_net: u64,
    pub tokens_out_gross: u64,
    pub tokens_out_net: u64,
    pub sol_reserve_after: u64,
    pub token_reserve_after: u64,
    pub lp_supply_after: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct SwapExecuted {
    pub pool: [u8; 32],
    pub user: [u8; 32],
    pub sol_source: [u8; 32],
    pub buy: bool,
    pub amount_in_gross: u64,
    pub amount_in_net: u64,
    pub amount_out_gross: u64,
    pub amount_out_net: u64,
    pub fee: u64,
    pub sol_reserve_after: u64,
    pub token_reserve_after: u64,
}

// ============================================================================
// On-chain event Borsh shapes — torch_market
// ============================================================================
//
// Mirror programs/torch_market/src/handlers/{token,market,migration,swap,short,
// lending,revival}.rs #[event] definitions. Field order is significant.

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct MarketCreated {
    pub mint: [u8; 32],
    pub creator: [u8; 32],
    pub name: String,
    pub symbol: String,
    pub metadata_uri: String,
    pub is_community_token: bool,
    pub sol_target: u64,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct BondingCurveTrade {
    pub mint: [u8; 32],
    pub trader: [u8; 32],
    pub vault: [u8; 32], // Pubkey::default() = direct trade (no vault routing)
    pub is_buy: bool,
    pub sol_in: u64,
    pub sol_out: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub sol_to_treasury: u64,
    pub sol_to_creator: u64,
    pub protocol_fee: u64,
    pub virtual_sol_after: u64,
    pub virtual_token_after: u64,
    pub real_sol_after: u64,
    pub real_token_after: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct MigratedToDex {
    pub mint: [u8; 32],
    pub deep_pool: [u8; 32],
    pub sol_seeded: u64,
    pub tokens_seeded: u64,
    pub lp_burned: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct VaultSwapExecuted {
    pub vault: [u8; 32],
    pub mint: [u8; 32],
    pub signer: [u8; 32],
    pub is_buy: bool,
    pub amount_in: u64,
    pub minimum_amount_out: u64,
}

// [V21] Leverage events. Field order MUST match the on-chain `#[event]` structs
// (borsh is positional). Pubkeys are [u8; 32]. Note `user`/`borrower` comes FIRST
// in V21 (V20 had `mint` first). For via_vault handlers these same events are
// emitted with `user`/`borrower` = the torch_vault PDA.
#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct OpenShortEvent {
    pub user: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub collateral_sol_gross: u64,
    pub open_fee_sol: u64,
    pub net_collateral_sol: u64,
    pub tokens_borrowed: u64,
    pub vault_sol: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct CloseShortEvent {
    pub user: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub debt_repaid: u64,
    pub sol_spent_on_buyback: u64,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub surplus_sol_to_user: u64,
    pub fully_closed: bool,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct LiquidateShortEvent {
    pub liquidator: [u8; 32],
    pub borrower: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub tokens_covered: u64,
    pub sol_seized: u64,
    pub bad_debt: u64,
    pub bonus_bps: u16,
    pub twap_ltv: u64,
    pub residual_sol_to_borrower: u64,
    pub fully_liquidated: bool,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct OpenLongEvent {
    pub user: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub collateral_tokens: u64,
    pub borrowed_sol_gross: u64,
    pub open_fee_sol: u64,
    pub atomic_buy_sol: u64,
    pub vault_tokens: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct CloseLongEvent {
    pub user: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub tokens_sold: u64,
    pub sol_out: u64,
    pub debt_repaid: u64,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub surplus_sol_to_user: u64,
    pub fully_closed: bool,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct LiquidateLongEvent {
    pub liquidator: [u8; 32],
    pub borrower: [u8; 32],
    pub mint: [u8; 32],
    pub position_index: u32,
    pub debt_covered: u64,
    pub tokens_seized: u64,
    pub bad_debt: u64,
    pub bonus_bps: u16,
    pub twap_ltv: u64,
    pub fully_liquidated: bool,
}

// [lifecycle] Emitted once, on the buy that crosses the bonding target.
#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct BondingCompleted {
    pub mint: [u8; 32],
    pub real_sol_reserves: u64,
    pub bonding_complete_slot: u64,
}

// [lifecycle] Emitted once when a failed token is reclaimed (curve SOL →
// protocol treasury).
#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct TokenReclaimed {
    pub mint: [u8; 32],
    pub sol_to_protocol_treasury: u64,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct RevivalContribution {
    pub mint: [u8; 32],
    pub contributor: [u8; 32],
    pub amount: u64,
    pub total_contributed: u64,
    pub threshold: u64,
    pub revived: bool,
}

#[derive(borsh::BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct TokenRevived {
    pub mint: [u8; 32],
    pub total_contributed: u64,
    pub revival_slot: u64,
}

// ============================================================================
// Decoded event envelopes
// ============================================================================

#[derive(Debug, Clone)]
pub enum DeepPoolEvent {
    PoolCreated(PoolCreated),
    SwapExecuted(SwapExecuted),
    LiquidityAdded(LiquidityAdded),
    LiquidityRemoved(LiquidityRemoved),
}

#[derive(Debug, Clone)]
pub enum TorchEvent {
    MarketCreated(MarketCreated),
    BondingCurveTrade(BondingCurveTrade),
    MigratedToDex(MigratedToDex),
    VaultSwapExecuted(VaultSwapExecuted),
    OpenShort(OpenShortEvent),
    CloseShort(CloseShortEvent),
    LiquidateShort(LiquidateShortEvent),
    OpenLong(OpenLongEvent),
    CloseLong(CloseLongEvent),
    LiquidateLong(LiquidateLongEvent),
    BondingCompleted(BondingCompleted),
    TokenReclaimed(TokenReclaimed),
    RevivalContribution(RevivalContribution),
    TokenRevived(TokenRevived),
}

#[derive(Debug, Clone)]
pub enum AnyEvent {
    DeepPool(DeepPoolEvent),
    Torch(TorchEvent),
}

// One decoded event with the tx context the writer needs to persist it.
#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub signature: String,
    // Index of the containing transaction WITHIN ITS BLOCK (Yellowstone block
    // order live; signature-walk order on backfill). The writer sorts each
    // block by (tx_idx, inner_ix_idx) so events apply in chain order —
    // sorting by signature reordered same-block events arbitrarily (I-1).
    // Not persisted: single-writer chain-ordered inserts make serial ids the
    // durable intra-slot order; read queries tiebreak on id.
    pub tx_idx: i32,
    pub inner_ix_idx: i32,
    pub slot: i64,
    pub block_time: Option<DateTime<Utc>>,
    pub event: AnyEvent,
    // Memo text co-resident with the trade ix in the same tx, if any. Captured
    // here so the writer can persist to `messages` with the same atomic
    // boundary as the trade row. None for non-trade events.
    pub memo: Option<String>,
    // [V21] True when the emitting outer instruction was a `*_via_vault`
    // leverage variant — i.e. the position's `owner` is a TorchVault PDA, not a
    // wallet. Resolved from the parent ix discriminator (the event payload
    // alone can't tell). Always false for non-leverage events.
    pub via_vault: bool,
}

// One block's worth of decoded events. The unit pushed from the gRPC
// subscriber to the writer; one BlockBatch = one Postgres transaction.
#[derive(Debug)]
pub struct BlockBatch {
    pub slot: u64,
    pub events: Vec<DecodedEvent>,
}

// ============================================================================
// DB enum types (sqlx-mapped to Postgres enums)
// ============================================================================

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "market_status", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum MarketStatus {
    // Lifecycle: BONDING → COMPLETE → MIGRATED, or BONDING → RECLAIMED →
    // (revival) → BONDING. Relabeled from RS/RD (+ dead ASN removed) 2026-06-09;
    // every transition now rides a program event (BondingCurveTrade /
    // BondingCompleted / MigratedToDex / TokenReclaimed / TokenRevived) — the
    // indexer never derives state the program didn't announce.
    Bonding,
    Complete,
    Migrated,
    Reclaimed,
}

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "market_tier", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MarketTier {
    // Spark (50 SOL) removed from the program — insufficient margin safety.
    Flame,
    Torch,
}

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "position_health", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PositionHealth {
    Healthy,
    AtRisk,
    Liquidatable,
    None,
}

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "position_side", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PositionSide {
    Long,
    Short,
}

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "position_event_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PositionEventKind {
    Open,
    Close,
    Liquidate,
}

#[derive(Debug, Clone, Copy, sqlx::Type, Serialize, Deserialize, PartialEq, Eq)]
#[sqlx(type_name = "metadata_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Ok,
    NotFound,
    Error,
}

// ============================================================================
// DB row types — deep_pool (verbatim from deep_pool/contracts.rs)
// ============================================================================

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct PoolRow {
    pub pool_id: i32,
    pub pubkey: String,
    pub config: String,
    pub token_mint: String,
    pub lp_mint: String,
    pub creator: String,
    pub sol_initial: i64,
    pub tokens_initial: i64,
    pub lp_supply_initial: i64,
    pub slot: i64,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct ReservesRow {
    pub reserve_id: i32,
    pub pool_id: i32,
    pub sol_reserve: i64,
    pub token_reserve: i64,
    pub lp_supply: i64,
    pub last_slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct SwapRow {
    pub swap_id: i32,
    pub pool_id: i32,
    pub user_pk: String,
    pub sol_source: String,
    pub is_buy: bool,
    pub amount_in_gross: i64,
    pub amount_in_net: i64,
    pub amount_out_gross: i64,
    pub amount_out_net: i64,
    pub fee: i64,
    pub sol_reserve_after: i64,
    pub token_reserve_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct LiquidityRow {
    pub liquidity_id: i32,
    pub pool_id: i32,
    pub provider: String,
    pub is_add: bool,
    pub sol_amount_gross: i64,
    pub sol_amount_net: i64,
    pub tokens_amount_gross: i64,
    pub tokens_amount_net: i64,
    pub lp_user_amount: i64,
    pub lp_locked: i64,
    pub lp_supply_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// DB row types — torch
// ============================================================================

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct MarketRow {
    pub mint: String,
    pub name: String,
    pub symbol: String,
    pub metadata_uri: Option<String>,
    pub image_url: Option<String>,
    pub creator: String,
    pub is_community_token: bool,
    pub status: MarketStatus,
    pub tier: MarketTier,
    pub sol_target: i64,
    pub virtual_sol: i64,
    pub virtual_token: i64,
    pub real_sol: i64,
    pub real_token: i64,
    pub created_at_slot: i64,
    pub bonding_complete_slot: Option<i64>,
    pub migrated_slot: Option<i64>,
    pub reclaimed_slot: Option<i64>,
    pub last_activity_slot: i64,
    pub deep_pool_pubkey: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct TradeRow {
    pub trade_id: i32,
    pub mint: String,
    pub trader: String,
    pub vault: Option<String>,
    pub is_buy: bool,
    pub sol_in: i64,
    pub sol_out: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub sol_to_treasury: i64,
    pub sol_to_creator: i64,
    pub protocol_fee: i64,
    pub virtual_sol_after: i64,
    pub virtual_token_after: i64,
    pub real_sol_after: i64,
    pub real_token_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct MessageRow {
    pub message_id: i32,
    pub mint: String,
    pub sender: String,
    pub memo_text: String,
    pub action_kind: Option<String>,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct PositionRow {
    pub mint: String,
    pub owner: String,
    pub side: PositionSide,
    pub position_index: i32,
    pub collateral_amount: i64,
    pub debt_amount: i64,
    pub open_fee_sol: i64,
    pub vault_balance: i64,
    pub accrued_interest_stored: i64,
    pub last_update_slot: i64,
    pub health: PositionHealth,
    pub is_active: bool,
    pub owner_is_vault: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct PositionEventRow {
    pub event_id: i64,
    pub mint: String,
    pub owner: String,
    pub side: PositionSide,
    pub position_index: i32,
    pub kind: PositionEventKind,
    pub liquidator: Option<String>,
    pub sol_in: Option<i64>,
    pub sol_out: Option<i64>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub interest_paid: Option<i64>,
    pub principal_paid: Option<i64>,
    pub surplus_sol: Option<i64>,
    pub bad_debt: Option<i64>,
    pub twap_ltv: Option<i64>,
    pub bonus_bps: Option<i32>,
    pub seized: Option<i64>,
    pub residual: Option<i64>,
    pub fully_resolved: Option<bool>,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct MigrationRow {
    pub mint: String,
    pub deep_pool_pubkey: String,
    pub sol_seeded: i64,
    pub tokens_seeded: i64,
    pub lp_burned: i64,
    pub slot: i64,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Insert shapes — deep_pool
// ============================================================================

#[derive(Debug, Clone)]
pub struct NewPoolRow {
    pub pubkey: String,
    pub config: String,
    pub token_mint: String,
    pub lp_mint: String,
    pub creator: String,
    pub sol_initial: i64,
    pub tokens_initial: i64,
    pub lp_supply_initial: i64,
    pub slot: i64,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewReservesRow {
    pub pool_id: i32,
    pub sol_reserve: i64,
    pub token_reserve: i64,
    pub lp_supply: i64,
    pub last_slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewSwapRow {
    pub pool_id: i32,
    pub user_pk: String,
    pub sol_source: String,
    pub is_buy: bool,
    pub amount_in_gross: i64,
    pub amount_in_net: i64,
    pub amount_out_gross: i64,
    pub amount_out_net: i64,
    pub fee: i64,
    pub sol_reserve_after: i64,
    pub token_reserve_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewLiquidityRow {
    pub pool_id: i32,
    pub provider: String,
    pub is_add: bool,
    pub sol_amount_gross: i64,
    pub sol_amount_net: i64,
    pub tokens_amount_gross: i64,
    pub tokens_amount_net: i64,
    pub lp_user_amount: i64,
    pub lp_locked: i64,
    pub lp_supply_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Insert shapes — torch
// ============================================================================

#[derive(Debug, Clone)]
pub struct NewMarketRow {
    pub mint: String,
    pub name: String,
    pub symbol: String,
    pub metadata_uri: Option<String>,
    pub creator: String,
    pub is_community_token: bool,
    pub status: MarketStatus,
    pub tier: MarketTier,
    pub sol_target: i64,
    pub virtual_sol: i64,
    pub virtual_token: i64,
    pub real_sol: i64,
    pub real_token: i64,
    pub created_at_slot: i64,
    pub last_activity_slot: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewTradeRow {
    pub mint: String,
    pub trader: String,
    pub vault: Option<String>,
    pub is_buy: bool,
    pub sol_in: i64,
    pub sol_out: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub sol_to_treasury: i64,
    pub sol_to_creator: i64,
    pub protocol_fee: i64,
    pub virtual_sol_after: i64,
    pub virtual_token_after: i64,
    pub real_sol_after: i64,
    pub real_token_after: i64,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMessageRow {
    pub mint: String,
    pub sender: String,
    pub memo_text: String,
    pub action_kind: Option<String>,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPositionRow {
    pub mint: String,
    pub owner: String,
    pub side: PositionSide,
    pub position_index: i32,
    pub collateral_amount: i64,
    pub debt_amount: i64,
    pub open_fee_sol: i64,
    pub vault_balance: i64,
    pub accrued_interest_stored: i64,
    pub last_update_slot: i64,
    pub health: PositionHealth,
    pub is_active: bool,
    pub owner_is_vault: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewPositionEventRow {
    pub mint: String,
    pub owner: String,
    pub side: PositionSide,
    pub position_index: i32,
    pub kind: PositionEventKind,
    pub liquidator: Option<String>,
    pub sol_in: Option<i64>,
    pub sol_out: Option<i64>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub interest_paid: Option<i64>,
    pub principal_paid: Option<i64>,
    pub surplus_sol: Option<i64>,
    pub bad_debt: Option<i64>,
    pub twap_ltv: Option<i64>,
    pub bonus_bps: Option<i32>,
    pub seized: Option<i64>,
    pub residual: Option<i64>,
    pub fully_resolved: Option<bool>,
    pub slot: i64,
    pub signature: String,
    pub inner_ix_idx: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMigrationRow {
    pub mint: String,
    pub deep_pool_pubkey: String,
    pub sol_seeded: i64,
    pub tokens_seeded: i64,
    pub lp_burned: i64,
    pub slot: i64,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Broadcast (post-COMMIT WS frames)
// ============================================================================
//
// The WS firehose carries the same wire format documented in
// docs/indexer.md §"WS firehose": one frame per persisted event, typed by
// kind. Clients filter client-side.

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BroadcastFrame {
    // Deep_pool
    Pool(Arc<PoolRow>),
    Swap(Arc<SwapRow>),
    Liquidity(Arc<LiquidityRow>),
    Reserves(Arc<ReservesRow>),
    // Torch
    Market(Arc<MarketRow>),
    Trade(Arc<TradeRow>),
    Message(Arc<MessageRow>),
    Position(Arc<PositionRow>),
    PositionEvent(Arc<PositionEventRow>),
    Migration(Arc<MigrationRow>),
}

// ============================================================================
// Runtime state shared between writer and API
// ============================================================================

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub broadcaster: Broadcaster,
}

#[derive(Clone, Debug)]
pub struct Broadcaster {
    sender: broadcast::Sender<BroadcastFrame>,
}

impl Broadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastFrame> {
        self.sender.subscribe()
    }

    pub fn publish(&self, frame: BroadcastFrame) {
        // send returns Err only when there are zero subscribers — that's fine.
        let _ = self.sender.send(frame);
    }

    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}
