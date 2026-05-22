// Domain: trades (torch). Bonding-curve trade events — buys + sells, both
// direct and via-vault, unified by `is_buy` + `vault IS NULL`.
//
// Append-only, idempotent on (signature, inner_ix_idx).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{NewTradeRow, TradeRow};

#[derive(Default, Debug, Clone)]
pub struct TradeFilter {
    pub ids: Option<Vec<i32>>,
    pub mint: Option<String>,
    pub trader: Option<String>,
    pub is_buy: Option<bool>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    trade_id: i32,
) -> sqlx::Result<Option<TradeRow>> {
    sqlx::query_as::<_, TradeRow>("SELECT * FROM trades WHERE trade_id = $1")
        .bind(trade_id)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: TradeFilter,
) -> sqlx::Result<Vec<TradeRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM trades WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND trade_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(trader) = filter.trader {
        qb.push(" AND trader = ").push_bind(trader);
    }
    if let Some(is_buy) = filter.is_buy {
        qb.push(" AND is_buy = ").push_bind(is_buy);
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    // Order by slot then disambiguators so cross-page same-slot rows are
    // deterministic (matches the reserves pattern).
    qb.push(" ORDER BY slot DESC, signature DESC, inner_ix_idx DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<TradeRow>().fetch_all(&mut **tx).await
}

pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewTradeRow],
) -> sqlx::Result<Vec<TradeRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO trades (
            mint, trader, vault, is_buy,
            sol_in, sol_out, tokens_in, tokens_out,
            sol_to_treasury, sol_to_creator, protocol_fee,
            virtual_sol_after, virtual_token_after, real_sol_after, real_token_after,
            slot, signature, inner_ix_idx, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(&row.mint)
            .push_bind(&row.trader)
            .push_bind(&row.vault)
            .push_bind(row.is_buy)
            .push_bind(row.sol_in)
            .push_bind(row.sol_out)
            .push_bind(row.tokens_in)
            .push_bind(row.tokens_out)
            .push_bind(row.sol_to_treasury)
            .push_bind(row.sol_to_creator)
            .push_bind(row.protocol_fee)
            .push_bind(row.virtual_sol_after)
            .push_bind(row.virtual_token_after)
            .push_bind(row.real_sol_after)
            .push_bind(row.real_token_after)
            .push_bind(row.slot)
            .push_bind(&row.signature)
            .push_bind(row.inner_ix_idx)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *");
    qb.build_query_as::<TradeRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM trades WHERE trade_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
