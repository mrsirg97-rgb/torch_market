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

use crate::contracts::{MarketRow, MarketStatus, MarketTier, NewMarketRow};

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

// Idempotent insert on `(mint)`. The torch program does not allow re-creating
// a market under the same mint, so the conflict path is only hit during
// backfill replay.
pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    row: &NewMarketRow,
) -> sqlx::Result<Option<MarketRow>> {
    sqlx::query_as::<_, MarketRow>(
        "INSERT INTO markets (
            mint, name, symbol, metadata_uri, creator, is_community_token,
            status, tier, sol_target,
            virtual_sol, virtual_token, real_sol, real_token,
            created_at_slot, last_activity_slot,
            created_at, updated_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $16
         )
         ON CONFLICT (mint) DO NOTHING
         RETURNING *",
    )
    .bind(&row.mint)
    .bind(&row.name)
    .bind(&row.symbol)
    .bind(&row.metadata_uri)
    .bind(&row.creator)
    .bind(row.is_community_token)
    .bind(row.status)
    .bind(row.tier)
    .bind(row.sol_target)
    .bind(row.virtual_sol)
    .bind(row.virtual_token)
    .bind(row.real_sol)
    .bind(row.real_token)
    .bind(row.created_at_slot)
    .bind(row.last_activity_slot)
    .bind(row.created_at)
    .fetch_optional(&mut **tx)
    .await
}

// Apply a BondingCurveTrade — updates the four reserves columns plus the
// last_activity_slot. Idempotent at the writer layer (trade rows are
// (signature, inner_ix_idx) keyed) so this is safe to retry.
#[allow(clippy::too_many_arguments)]
pub async fn apply_trade(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    virtual_sol: i64,
    virtual_token: i64,
    real_sol: i64,
    real_token: i64,
    last_activity_slot: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE markets
         SET virtual_sol = $2,
             virtual_token = $3,
             real_sol = $4,
             real_token = $5,
             last_activity_slot = $6,
             updated_at = $7
         WHERE mint = $1",
    )
    .bind(mint)
    .bind(virtual_sol)
    .bind(virtual_token)
    .bind(real_sol)
    .bind(real_token)
    .bind(last_activity_slot)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// Apply MigratedToDex — mirrors the on-chain migrate_to_dex_handler:
//   - status → MIGRATED
//   - migrated_slot set
//   - deep_pool_pubkey FK set
//   - real_sol / real_token zeroed (the on-chain handler does this; without
//     this here, the indexer's row keeps stale reserves and the frontend's
//     `progress = real_sol / sol_target` math stays pegged at 100% even
//     after the market is no longer on the bonding curve)
//
// The pool row in `pools` must already exist when this runs. In live ingest
// the writer's phase-2 inserts deep_pool's PoolCreated before phase-3
// touches MigratedToDex (same block batch, same writer txn). In backfill,
// deep_pool is walked before torch — same ordering guarantee.
pub async fn apply_migration(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    deep_pool_pubkey: &str,
    migrated_slot: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE markets
         SET status = 'MIGRATED',
             migrated_slot = $2,
             deep_pool_pubkey = $3,
             real_sol = 0,
             real_token = 0,
             last_activity_slot = $2,
             updated_at = $4
         WHERE mint = $1",
    )
    .bind(mint)
    .bind(migrated_slot)
    .bind(deep_pool_pubkey)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// Status transition helper for state changes that don't have a dedicated
// apply_*: RS→RD on bonding complete, RD→RECLAIMED on reclaim flow.
pub async fn mark_status(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    status: MarketStatus,
    slot: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    // Track the slot specific to the new status so the API can render
    // lifecycle timestamps without reading the whole event log.
    let column = match status {
        MarketStatus::Rd => "bonding_complete_slot",
        MarketStatus::Reclaimed => "reclaimed_slot",
        _ => "last_activity_slot",
    };
    let sql = format!(
        "UPDATE markets
         SET status = $2, {column} = $3, last_activity_slot = $3, updated_at = $4
         WHERE mint = $1",
    );
    sqlx::query(&sql)
        .bind(mint)
        .bind(status)
        .bind(slot)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

// Image URL + metadata fields populated by the async metadata fetcher.
// Separate from the on-chain event flow because Arweave/Irys lookups are
// off-chain HTTP fetches that may fail or be slow.
pub async fn set_metadata(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    image_url: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE markets
         SET image_url = $2, updated_at = $3
         WHERE mint = $1",
    )
    .bind(mint)
    .bind(image_url)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
