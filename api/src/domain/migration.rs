// Domain: migrations (torch). One row per migrated mint. The `markets` row
// also tracks migration status — `migrations` is the event-log audit trail
// (when did the migration tx land, what was the LP burned).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::MigrationRow;

#[derive(Default, Debug, Clone)]
pub struct MigrationFilter {
    pub mint: Option<String>,
    pub deep_pool_pubkey: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get_by_mint(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
) -> sqlx::Result<Option<MigrationRow>> {
    sqlx::query_as::<_, MigrationRow>("SELECT * FROM migrations WHERE mint = $1")
        .bind(mint)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: MigrationFilter,
) -> sqlx::Result<Vec<MigrationRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM migrations WHERE 1=1");
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(deep_pool_pubkey) = filter.deep_pool_pubkey {
        qb.push(" AND deep_pool_pubkey = ").push_bind(deep_pool_pubkey);
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    qb.push(" ORDER BY slot DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<MigrationRow>().fetch_all(&mut **tx).await
}
