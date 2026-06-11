// Domain: pools (deep_pool).
//
// Thin CRUD over the `pools` table. No cache, no composition — services own
// those concerns. All methods take a borrowed transaction so the caller
// controls atomicity and isolation. Ported verbatim from deep_pool/indexer.

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::PoolRow;

#[derive(Default, Debug, Clone)]
pub struct PoolFilter {
    pub ids: Option<Vec<i32>>,
    pub pubkeys: Option<Vec<String>>,
    pub token_mints: Option<Vec<String>>,
    pub creators: Option<Vec<String>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    pool_id: i32,
) -> sqlx::Result<Option<PoolRow>> {
    sqlx::query_as::<_, PoolRow>("SELECT * FROM pools WHERE pool_id = $1")
        .bind(pool_id)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: PoolFilter,
) -> sqlx::Result<Vec<PoolRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM pools WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND pool_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(pubkeys) = filter.pubkeys {
        qb.push(" AND pubkey = ANY(").push_bind(pubkeys).push(")");
    }
    if let Some(token_mints) = filter.token_mints {
        qb.push(" AND token_mint = ANY(").push_bind(token_mints).push(")");
    }
    if let Some(creators) = filter.creators {
        qb.push(" AND creator = ANY(").push_bind(creators).push(")");
    }
    qb.push(" ORDER BY pool_id");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<PoolRow>().fetch_all(&mut **tx).await
}
