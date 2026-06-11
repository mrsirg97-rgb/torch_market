// Domain: reserves (deep_pool). Append-only event-stream of pool reserve
// snapshots. "Current" reserves for a pool = the latest by (last_slot,
// reserve_id). Idempotent on (signature, inner_ix_idx).
//
// Ported verbatim from deep_pool/indexer.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{NewReservesRow, ReservesRow};

#[derive(Default, Debug, Clone)]
pub struct ReservesFilter {
    pub ids: Option<Vec<i32>>,
    pub pool_ids: Option<Vec<i32>>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

// Idempotent batch insert keyed on (signature, inner_ix_idx).
pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewReservesRow],
) -> sqlx::Result<Vec<ReservesRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO reserves (
            pool_id, sol_reserve, token_reserve, lp_supply, last_slot,
            signature, inner_ix_idx, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(row.pool_id)
            .push_bind(row.sol_reserve)
            .push_bind(row.token_reserve)
            .push_bind(row.lp_supply)
            .push_bind(row.last_slot)
            .push_bind(&row.signature)
            .push_bind(row.inner_ix_idx)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *");
    qb.build_query_as::<ReservesRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM reserves WHERE reserve_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
