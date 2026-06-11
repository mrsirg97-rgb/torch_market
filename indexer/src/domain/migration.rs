// Domain: migrations (torch). One row per migrated mint. The `markets` row
// also tracks migration status — `migrations` is the event-log audit trail
// (when did the migration tx land, what was the LP burned).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};

use crate::contracts::{MigrationRow, NewMigrationRow};

#[derive(Default, Debug, Clone)]
pub struct MigrationFilter {
    pub mint: Option<String>,
    pub deep_pool_pubkey: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    row: &NewMigrationRow,
) -> sqlx::Result<Option<MigrationRow>> {
    sqlx::query_as::<_, MigrationRow>(
        "INSERT INTO migrations (
            mint, deep_pool_pubkey, sol_seeded, tokens_seeded, lp_burned,
            slot, signature, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (mint) DO NOTHING
         RETURNING *",
    )
    .bind(&row.mint)
    .bind(&row.deep_pool_pubkey)
    .bind(row.sol_seeded)
    .bind(row.tokens_seeded)
    .bind(row.lp_burned)
    .bind(row.slot)
    .bind(&row.signature)
    .bind(row.created_at)
    .fetch_optional(&mut **tx)
    .await
}
