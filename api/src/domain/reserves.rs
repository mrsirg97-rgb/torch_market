// Domain: reserves (deep_pool). Append-only event-stream of pool reserve
// snapshots. "Current" reserves for a pool = the latest by (last_slot,
// reserve_id). Idempotent on (signature, inner_ix_idx).
//
// Ported verbatim from deep_pool/indexer.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::ReservesRow;

#[derive(Default, Debug, Clone)]
pub struct ReservesFilter {
    pub ids: Option<Vec<i32>>,
    pub pool_ids: Option<Vec<i32>>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    reserve_id: i32,
) -> sqlx::Result<Option<ReservesRow>> {
    sqlx::query_as::<_, ReservesRow>("SELECT * FROM reserves WHERE reserve_id = $1")
        .bind(reserve_id)
        .fetch_optional(&mut **tx)
        .await
}

// Latest reserves snapshot per pool. Tiebreak on (signature DESC,
// inner_ix_idx DESC) instead of reserve_id so cross-page same-slot events
// from backfill are deterministic.
pub async fn latest_for_pools(
    tx: &mut Transaction<'_, Postgres>,
    pool_ids: &[i32],
) -> sqlx::Result<Vec<ReservesRow>> {
    if pool_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, ReservesRow>(
        "SELECT DISTINCT ON (pool_id) *
         FROM reserves
         WHERE pool_id = ANY($1)
         ORDER BY pool_id, last_slot DESC, reserve_id DESC",
    )
    .bind(pool_ids)
    .fetch_all(&mut **tx)
    .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: ReservesFilter,
) -> sqlx::Result<Vec<ReservesRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM reserves WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND reserve_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(pool_ids) = filter.pool_ids {
        qb.push(" AND pool_id = ANY(").push_bind(pool_ids).push(")");
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    qb.push(" ORDER BY last_slot DESC, reserve_id DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<ReservesRow>().fetch_all(&mut **tx).await
}
