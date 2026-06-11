// Postgres pool wiring + a couple of cross-table helpers that don't fit any
// single domain module.
//
// Schema is bootstrapped from db/01-schema.sql + db/02-indexer-role.sh at
// first Postgres boot. The indexer connects with read+write privileges on
// all torch and deep_pool tables; no migration runner is wired in here —
// schema evolution is handled by rebuilding the volume during development.

use sqlx::{postgres::PgPoolOptions, PgPool};

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
        .map_err(Into::into)
}

// Cold start (no row) returns 0 — the subscriber treats that as "stream from
// current tip" and the first committed block seeds the checkpoint. Single
// cursor across both program subscriptions since Yellowstone is slot-ordered
// globally across all subscribed programs.
pub async fn last_processed_slot(pool: &PgPool) -> anyhow::Result<u64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT last_processed_slot FROM indexer_state WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(s,)| s as u64).unwrap_or(0))
}
