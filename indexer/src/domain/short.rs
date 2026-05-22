// Domain: shorts (torch). Current-state, composite PK (mint, shorter), UPSERT.
// tokens_borrowed is the NET amount (post-Token-2022-fee), matching the
// on-chain field after the open_short net-recording fix.

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{NewShortRow, PositionHealth, ShortRow};

#[derive(Default, Debug, Clone)]
pub struct ShortFilter {
    pub mint: Option<String>,
    pub shorter: Option<String>,
    pub health: Option<PositionHealth>,
    pub is_active: Option<bool>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    shorter: &str,
) -> sqlx::Result<Option<ShortRow>> {
    sqlx::query_as::<_, ShortRow>("SELECT * FROM shorts WHERE mint = $1 AND shorter = $2")
        .bind(mint)
        .bind(shorter)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: ShortFilter,
) -> sqlx::Result<Vec<ShortRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM shorts WHERE 1=1");
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(shorter) = filter.shorter {
        qb.push(" AND shorter = ").push_bind(shorter);
    }
    if let Some(health) = filter.health {
        qb.push(" AND health = ").push_bind(health);
    }
    if let Some(is_active) = filter.is_active {
        qb.push(" AND is_active = ").push_bind(is_active);
    }
    qb.push(" ORDER BY updated_at DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<ShortRow>().fetch_all(&mut **tx).await
}

pub async fn upsert(
    tx: &mut Transaction<'_, Postgres>,
    row: &NewShortRow,
) -> sqlx::Result<ShortRow> {
    sqlx::query_as::<_, ShortRow>(
        "INSERT INTO shorts (
            mint, shorter, sol_collateral, tokens_borrowed,
            accrued_interest_stored, last_update_slot,
            health, is_active, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (mint, shorter) DO UPDATE SET
             sol_collateral = EXCLUDED.sol_collateral,
             tokens_borrowed = EXCLUDED.tokens_borrowed,
             accrued_interest_stored = EXCLUDED.accrued_interest_stored,
             last_update_slot = EXCLUDED.last_update_slot,
             health = EXCLUDED.health,
             is_active = EXCLUDED.is_active,
             updated_at = EXCLUDED.updated_at
         RETURNING *",
    )
    .bind(&row.mint)
    .bind(&row.shorter)
    .bind(row.sol_collateral)
    .bind(row.tokens_borrowed)
    .bind(row.accrued_interest_stored)
    .bind(row.last_update_slot)
    .bind(row.health)
    .bind(row.is_active)
    .bind(row.created_at)
    .bind(row.updated_at)
    .fetch_one(&mut **tx)
    .await
}
