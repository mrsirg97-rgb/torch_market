// Domain: swaps (deep_pool). Ported verbatim from deep_pool/indexer.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::SwapRow;

#[derive(Default, Debug, Clone)]
pub struct SwapFilter {
    pub ids: Option<Vec<i32>>,
    pub pool_ids: Option<Vec<i32>>,
    pub users: Option<Vec<String>>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    swap_id: i32,
) -> sqlx::Result<Option<SwapRow>> {
    sqlx::query_as::<_, SwapRow>("SELECT * FROM swaps WHERE swap_id = $1")
        .bind(swap_id)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: SwapFilter,
) -> sqlx::Result<Vec<SwapRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM swaps WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND swap_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(pool_ids) = filter.pool_ids {
        qb.push(" AND pool_id = ANY(").push_bind(pool_ids).push(")");
    }
    if let Some(users) = filter.users {
        qb.push(" AND user_pk = ANY(").push_bind(users).push(")");
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    qb.push(" ORDER BY slot DESC, swap_id DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<SwapRow>().fetch_all(&mut **tx).await
}
