// Domain: markets (torch).
//
// Current-state table — one row per mint, mutated by every event that
// changes bonding-curve reserves, status, or migration. The writer calls:
//   - `set()` on MarketCreated
//   - `apply_trade()` on BondingCurveTrade
//   - `apply_migration()` on MigratedToDex
//   - `mark_status()` for explicit RS→RD / RD→ASN / →RECLAIMED transitions
//
// `set_metadata()` is called by the async metadata-fetch sidecar once an
// Arweave/Irys URI resolves successfully.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{MarketRow, MarketStatus, MarketTier};

#[derive(Default, Debug, Clone)]
pub struct MarketFilter {
    pub mints: Option<Vec<String>>,
    pub status: Option<MarketStatus>,
    pub tier: Option<MarketTier>,
    pub creator: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get_by_mint(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
) -> sqlx::Result<Option<MarketRow>> {
    sqlx::query_as::<_, MarketRow>("SELECT * FROM markets WHERE mint = $1")
        .bind(mint)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: MarketFilter,
) -> sqlx::Result<Vec<MarketRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM markets WHERE 1=1");
    if let Some(mints) = filter.mints {
        qb.push(" AND mint = ANY(").push_bind(mints).push(")");
    }
    if let Some(status) = filter.status {
        qb.push(" AND status = ").push_bind(status);
    }
    if let Some(tier) = filter.tier {
        qb.push(" AND tier = ").push_bind(tier);
    }
    if let Some(creator) = filter.creator {
        qb.push(" AND creator = ").push_bind(creator);
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    // Sort newest-active-first by default; the /markets browse page wants
    // recent traders surfaced over inactive long-tail markets.
    qb.push(" ORDER BY last_activity_slot DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<MarketRow>().fetch_all(&mut **tx).await
}
