// Domain: trades (torch). Bonding-curve trade events — buys + sells, both
// direct and via-vault, unified by `is_buy` + `vault IS NULL`.
//
// Append-only, idempotent on (signature, inner_ix_idx).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::TradeRow;

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
    // [I-1] Serial id = intra-slot chain order (single chain-ordered writer).
    qb.push(" ORDER BY slot DESC, trade_id DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<TradeRow>().fetch_all(&mut **tx).await
}
