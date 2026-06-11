// Domain: positions (torch, V21). Unified current-state table for leverage
// positions — one on-chain `Position` struct (side = long | short), keyed by
// (mint, owner, side, position_index). UPSERTed on every open/close/liquidate.
// Closed/liquidated positions remain as rows with is_active=false (audit
// trail); never DELETE. `owner` may be a wallet or a TorchVault PDA
// (owner_is_vault). Companion append-only log lives in the `event` submodule.

use sqlx::{Postgres, Transaction};

use crate::contracts::{
    NewPositionRow, PositionEventKind, PositionHealth, PositionRow, PositionSide,
};

#[derive(Default, Debug, Clone)]
pub struct PositionFilter {
    pub mint: Option<String>,
    pub owner: Option<String>,
    pub side: Option<PositionSide>,
    pub health: Option<PositionHealth>,
    pub is_active: Option<bool>,
    pub limit: Option<i64>,
}
// Atomic-write internal (reconcile/cache) — duplicated in /api for queries.

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    owner: &str,
    side: PositionSide,
    position_index: i32,
) -> sqlx::Result<Option<PositionRow>> {
    sqlx::query_as::<_, PositionRow>(
        "SELECT * FROM positions
         WHERE mint = $1 AND owner = $2 AND side = $3 AND position_index = $4",
    )
    .bind(mint)
    .bind(owner)
    .bind(side)
    .bind(position_index)
    .fetch_optional(&mut **tx)
    .await
}

// UPSERT on (mint, owner, side, position_index). On conflict every mutable
// field updates; PK + created_at + owner_is_vault are preserved. Returns the
// resulting row.
pub async fn upsert(
    tx: &mut Transaction<'_, Postgres>,
    row: &NewPositionRow,
) -> sqlx::Result<PositionRow> {
    sqlx::query_as::<_, PositionRow>(
        "INSERT INTO positions (
            mint, owner, side, position_index,
            collateral_amount, debt_amount, open_fee_sol, vault_balance,
            accrued_interest_stored, last_update_slot,
            health, is_active, owner_is_vault, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (mint, owner, side, position_index) DO UPDATE SET
             collateral_amount = EXCLUDED.collateral_amount,
             debt_amount = EXCLUDED.debt_amount,
             open_fee_sol = EXCLUDED.open_fee_sol,
             vault_balance = EXCLUDED.vault_balance,
             accrued_interest_stored = EXCLUDED.accrued_interest_stored,
             last_update_slot = EXCLUDED.last_update_slot,
             health = EXCLUDED.health,
             is_active = EXCLUDED.is_active,
             updated_at = EXCLUDED.updated_at
         RETURNING *",
    )
    .bind(&row.mint)
    .bind(&row.owner)
    .bind(row.side)
    .bind(row.position_index)
    .bind(row.collateral_amount)
    .bind(row.debt_amount)
    .bind(row.open_fee_sol)
    .bind(row.vault_balance)
    .bind(row.accrued_interest_stored)
    .bind(row.last_update_slot)
    .bind(row.health)
    .bind(row.is_active)
    .bind(row.owner_is_vault)
    .bind(row.created_at)
    .bind(row.updated_at)
    .fetch_one(&mut **tx)
    .await
}

// Append-only leverage event log (`position_events`) — analog of `trades`.
// Preserves per-event analytics (bad_debt, twap_ltv, bonus_bps, seized) that
// the current-state `positions` table can't retain. Idempotent on
// (signature, inner_ix_idx) so backfill re-runs are safe.
pub mod event {
    use super::*;
    use crate::contracts::{NewPositionEventRow, PositionEventRow};

    #[derive(Default, Debug, Clone)]
    pub struct PositionEventFilter {
        pub mint: Option<String>,
        pub owner: Option<String>,
        pub side: Option<PositionSide>,
        pub kind: Option<PositionEventKind>,
        pub limit: Option<i64>,
    }


    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        row: &NewPositionEventRow,
    ) -> sqlx::Result<Option<PositionEventRow>> {
        sqlx::query_as::<_, PositionEventRow>(
            "INSERT INTO position_events (
                mint, owner, side, position_index, kind, liquidator,
                sol_in, sol_out, tokens_in, tokens_out,
                interest_paid, principal_paid, surplus_sol,
                bad_debt, twap_ltv, bonus_bps, seized, residual, fully_resolved,
                slot, signature, inner_ix_idx, created_at
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
             )
             ON CONFLICT (signature, inner_ix_idx) DO NOTHING
             RETURNING *",
        )
        .bind(&row.mint)
        .bind(&row.owner)
        .bind(row.side)
        .bind(row.position_index)
        .bind(row.kind)
        .bind(&row.liquidator)
        .bind(row.sol_in)
        .bind(row.sol_out)
        .bind(row.tokens_in)
        .bind(row.tokens_out)
        .bind(row.interest_paid)
        .bind(row.principal_paid)
        .bind(row.surplus_sol)
        .bind(row.bad_debt)
        .bind(row.twap_ltv)
        .bind(row.bonus_bps)
        .bind(row.seized)
        .bind(row.residual)
        .bind(row.fully_resolved)
        .bind(row.slot)
        .bind(&row.signature)
        .bind(row.inner_ix_idx)
        .bind(row.created_at)
        .fetch_optional(&mut **tx)
        .await
    }
}
