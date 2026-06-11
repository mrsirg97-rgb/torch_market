// Domain: pools (deep_pool).
//
// Thin CRUD over the `pools` table. No cache, no composition — services own
// those concerns. All methods take a borrowed transaction so the caller
// controls atomicity and isolation. Ported verbatim from deep_pool/indexer.

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{NewPoolRow, PoolRow};

#[derive(Default, Debug, Clone)]
pub struct PoolFilter {
    pub ids: Option<Vec<i32>>,
    pub pubkeys: Option<Vec<String>>,
    pub token_mints: Option<Vec<String>>,
    pub creators: Option<Vec<String>>,
    pub limit: Option<i64>,
}
// Atomic-write internal (reconcile/cache) — duplicated in /api for queries.

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

// Idempotent batch insert — returns only newly inserted rows. Rows that hit
// the (pubkey) UNIQUE constraint produce no output. Callers needing the
// existing pool_id for a duplicate should follow up with `list()`.
pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewPoolRow],
) -> sqlx::Result<Vec<PoolRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO pools (
            pubkey, config, token_mint, lp_mint, creator,
            sol_initial, tokens_initial, lp_supply_initial,
            slot, signature, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(&row.pubkey)
            .push_bind(&row.config)
            .push_bind(&row.token_mint)
            .push_bind(&row.lp_mint)
            .push_bind(&row.creator)
            .push_bind(row.sol_initial)
            .push_bind(row.tokens_initial)
            .push_bind(row.lp_supply_initial)
            .push_bind(row.slot)
            .push_bind(&row.signature)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (pubkey) DO NOTHING RETURNING *");
    qb.build_query_as::<PoolRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM pools WHERE pool_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
