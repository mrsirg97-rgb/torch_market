// Domain: liquidity_events (deep_pool). Ported verbatim from deep_pool/indexer.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{LiquidityRow, NewLiquidityRow};

#[derive(Default, Debug, Clone)]
pub struct LiquidityFilter {
    pub ids: Option<Vec<i32>>,
    pub pool_ids: Option<Vec<i32>>,
    pub providers: Option<Vec<String>>,
    pub is_add: Option<bool>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    liquidity_id: i32,
) -> sqlx::Result<Option<LiquidityRow>> {
    sqlx::query_as::<_, LiquidityRow>("SELECT * FROM liquidity_events WHERE liquidity_id = $1")
        .bind(liquidity_id)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: LiquidityFilter,
) -> sqlx::Result<Vec<LiquidityRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM liquidity_events WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND liquidity_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(pool_ids) = filter.pool_ids {
        qb.push(" AND pool_id = ANY(").push_bind(pool_ids).push(")");
    }
    if let Some(providers) = filter.providers {
        qb.push(" AND provider = ANY(").push_bind(providers).push(")");
    }
    if let Some(is_add) = filter.is_add {
        qb.push(" AND is_add = ").push_bind(is_add);
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    qb.push(" ORDER BY slot DESC, liquidity_id DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<LiquidityRow>().fetch_all(&mut **tx).await
}

// Idempotent batch insert keyed on (signature, inner_ix_idx).
pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewLiquidityRow],
) -> sqlx::Result<Vec<LiquidityRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO liquidity_events (
            pool_id, provider, is_add,
            sol_amount_gross, sol_amount_net,
            tokens_amount_gross, tokens_amount_net,
            lp_user_amount, lp_locked, lp_supply_after,
            slot, signature, inner_ix_idx, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(row.pool_id)
            .push_bind(&row.provider)
            .push_bind(row.is_add)
            .push_bind(row.sol_amount_gross)
            .push_bind(row.sol_amount_net)
            .push_bind(row.tokens_amount_gross)
            .push_bind(row.tokens_amount_net)
            .push_bind(row.lp_user_amount)
            .push_bind(row.lp_locked)
            .push_bind(row.lp_supply_after)
            .push_bind(row.slot)
            .push_bind(&row.signature)
            .push_bind(row.inner_ix_idx)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *");
    qb.build_query_as::<LiquidityRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM liquidity_events WHERE liquidity_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
