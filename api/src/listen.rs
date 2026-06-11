// Postgres LISTEN bridge (prompt-003): the single-writer ingest fires a thin
// pg_notify ({"t": table, "k": key}) INSIDE each write transaction; Postgres
// delivers on COMMIT, in commit order. This task fetches the row, assembles
// the typed frame, and routes it to rooms:
//   room(mint)  ← every frame for that market
//   AllMarkets  ← market-row updates + trade ticks only (index-page heartbeat)
//
// NOTIFY is not durable. Recovery on any listener reconnect:
//   - serial tables (trades/swaps/…): replay WHERE id > cursor (the serial
//     ids are intra-slot chain order — the I-1 invariant doubles as a resume
//     token)
//   - state tables (markets/positions/pools): no serial — publish a Resync
//     control frame; clients refetch what they render (their existing
//     reconnect contract).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::contracts::{
    LiquidityRow, MarketRow, MessageRow, MigrationRow, PoolRow, PositionEventRow, PositionRow,
    ReservesRow, SwapRow, TradeRow,
};
use crate::state::AppState;
use crate::ws::{BroadcastFrame, RoomKey};

#[derive(Debug, Deserialize)]
struct Thin {
    t: String,
    k: String,
}

#[derive(Default)]
struct Cursors {
    trades: i64,
    swaps: i64,
    liquidity: i64,
    reserves: i64,
    messages: i64,
    position_events: i64,
}

pub async fn run_listener(state: AppState, db_url: String) -> anyhow::Result<()> {
    let mut pool_mints: HashMap<i32, String> = HashMap::new();
    let mut cursors = Cursors::default();
    let mut first_connect = true;

    loop {
        let mut listener = match PgListener::connect(&db_url).await {
            Ok(l) => l,
            Err(e) => {
                warn!(error = %e, "pg listener connect failed; retrying in 2s");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };
        if let Err(e) = listener.listen("torch_events").await {
            warn!(error = %e, "LISTEN failed; retrying in 2s");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }
        info!("listening on torch_events");

        if first_connect {
            // Initialize cursors at current maxima — no replay of history.
            if let Err(e) = init_cursors(&state.pool, &mut cursors).await {
                warn!(error = %e, "cursor init failed");
            }
            first_connect = false;
        } else {
            // Reconnect: we may have missed notifications. Replay serial
            // tables from cursors; Resync-frame the state tables.
            crate::metrics::METRICS.listen_resyncs_total.inc();
            if let Err(e) = replay_serial_gaps(&state, &mut pool_mints, &mut cursors).await {
                warn!(error = %e, "gap replay failed");
            }
            state.rooms.publish_resync_all();
        }

        loop {
            match listener.recv().await {
                Ok(n) => {
                    crate::metrics::METRICS.notifies_received_total.inc();
                    if let Ok(thin) = serde_json::from_str::<Thin>(n.payload()) {
                        if let Err(e) =
                            handle_thin(&state, &mut pool_mints, &mut cursors, &thin).await
                        {
                            warn!(t = %thin.t, k = %thin.k, error = %e, "notify handling failed");
                        }
                    } else {
                        warn!(payload = %n.payload(), "unparseable notify payload");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "listener connection lost; reconnecting");
                    break; // outer loop reconnects + resyncs
                }
            }
        }
    }
}

async fn init_cursors(pool: &PgPool, c: &mut Cursors) -> sqlx::Result<()> {
    async fn maxid(pool: &PgPool, q: &str) -> sqlx::Result<i64> {
        let v: Option<i64> = sqlx::query_scalar(q).fetch_one(pool).await?;
        Ok(v.unwrap_or(0))
    }
    c.trades = maxid(pool, "SELECT MAX(trade_id)::bigint FROM trades").await?;
    c.swaps = maxid(pool, "SELECT MAX(swap_id)::bigint FROM swaps").await?;
    c.liquidity = maxid(pool, "SELECT MAX(liquidity_id)::bigint FROM liquidity_events").await?;
    c.reserves = maxid(pool, "SELECT MAX(reserve_id)::bigint FROM reserves").await?;
    c.messages = maxid(pool, "SELECT MAX(message_id)::bigint FROM messages").await?;
    c.position_events = maxid(pool, "SELECT MAX(event_id) FROM position_events").await?;
    Ok(())
}

// Resolve a pool_id to its token mint (cached; pools are immutable rows).
async fn pool_mint(
    pool: &PgPool,
    cache: &mut HashMap<i32, String>,
    pool_id: i32,
) -> sqlx::Result<Option<String>> {
    if let Some(m) = cache.get(&pool_id) {
        return Ok(Some(m.clone()));
    }
    let mint: Option<String> =
        sqlx::query_scalar("SELECT token_mint FROM pools WHERE pool_id = $1")
            .bind(pool_id)
            .fetch_optional(pool)
            .await?;
    if let Some(m) = &mint {
        cache.insert(pool_id, m.clone());
    }
    Ok(mint)
}

fn route_market(state: &AppState, mint: &str, frame: BroadcastFrame, also_all: bool) {
    let f = Arc::new(frame);
    state
        .rooms
        .publish(&RoomKey::Market(mint.to_string()), f.clone());
    if also_all {
        state.rooms.publish(&RoomKey::AllMarkets, f);
    }
}

async fn handle_thin(
    state: &AppState,
    pool_mints: &mut HashMap<i32, String>,
    cursors: &mut Cursors,
    thin: &Thin,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    match thin.t.as_str() {
        "markets" => {
            let row: Option<MarketRow> =
                sqlx::query_as("SELECT * FROM markets WHERE mint = $1")
                    .bind(&thin.k)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                // Market updates feed the index page too (status, progress).
                route_market(state, &thin.k, BroadcastFrame::Market(Arc::new(r)), true);
            }
        }
        "trades" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<TradeRow> =
                sqlx::query_as("SELECT * FROM trades WHERE trade_id = $1::int4")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.trades = cursors.trades.max(id);
                let mint = r.mint.clone();
                // Trade ticks feed AllMarkets (live sort heartbeats).
                route_market(state, &mint, BroadcastFrame::Trade(Arc::new(r)), true);
            }
        }
        "messages" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<MessageRow> =
                sqlx::query_as("SELECT * FROM messages WHERE message_id = $1::int4")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.messages = cursors.messages.max(id);
                let mint = r.mint.clone();
                route_market(state, &mint, BroadcastFrame::Message(Arc::new(r)), false);
            }
        }
        "position_events" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<PositionEventRow> =
                sqlx::query_as("SELECT * FROM position_events WHERE event_id = $1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.position_events = cursors.position_events.max(id);
                let mint = r.mint.clone();
                route_market(state, &mint, BroadcastFrame::PositionEvent(Arc::new(r)), false);
            }
        }
        "positions" => {
            // Composite key "mint|owner|side|index" — fetch by parts.
            let parts: Vec<&str> = thin.k.split('|').collect();
            if parts.len() == 4 {
                let side = parts[2].to_lowercase();
                let row: Option<PositionRow> = sqlx::query_as(
                    "SELECT * FROM positions WHERE mint = $1 AND owner = $2 AND side = $3::position_side AND position_index = $4",
                )
                .bind(parts[0])
                .bind(parts[1])
                .bind(side)
                .bind(parts[3].parse::<i32>()?)
                .fetch_optional(pool)
                .await?;
                if let Some(r) = row {
                    let mint = r.mint.clone();
                    route_market(state, &mint, BroadcastFrame::Position(Arc::new(r)), false);
                }
            }
        }
        "migrations" => {
            let row: Option<MigrationRow> =
                sqlx::query_as("SELECT * FROM migrations WHERE mint = $1")
                    .bind(&thin.k)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                let mint = r.mint.clone();
                route_market(state, &mint, BroadcastFrame::Migration(Arc::new(r)), true);
            }
        }
        "pools" => {
            let id: i32 = thin.k.parse()?;
            let row: Option<PoolRow> =
                sqlx::query_as("SELECT * FROM pools WHERE pool_id = $1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                pool_mints.insert(r.pool_id, r.token_mint.clone());
                let mint = r.token_mint.clone();
                route_market(state, &mint, BroadcastFrame::Pool(Arc::new(r)), false);
            }
        }
        "swaps" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<SwapRow> =
                sqlx::query_as("SELECT * FROM swaps WHERE swap_id = $1::int4")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.swaps = cursors.swaps.max(id);
                if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
                    // Post-migration trading: swaps ARE the trade ticks.
                    route_market(state, &mint, BroadcastFrame::Swap(Arc::new(r)), true);
                }
            }
        }
        "reserves" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<ReservesRow> =
                sqlx::query_as("SELECT * FROM reserves WHERE reserve_id = $1::int4")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.reserves = cursors.reserves.max(id);
                if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
                    route_market(state, &mint, BroadcastFrame::Reserves(Arc::new(r)), false);
                }
            }
        }
        "liquidity_events" => {
            let id: i64 = thin.k.parse()?;
            let row: Option<LiquidityRow> =
                sqlx::query_as("SELECT * FROM liquidity_events WHERE liquidity_id = $1::int4")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(r) = row {
                cursors.liquidity = cursors.liquidity.max(id);
                if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
                    route_market(state, &mint, BroadcastFrame::Liquidity(Arc::new(r)), false);
                }
            }
        }
        other => warn!(table = other, "unknown notify table"),
    }
    Ok(())
}

// Reconnect gap recovery for serial tables: fetch and route everything past
// each cursor, in id (= chain) order.
async fn replay_serial_gaps(
    state: &AppState,
    pool_mints: &mut HashMap<i32, String>,
    cursors: &mut Cursors,
) -> anyhow::Result<()> {
    let pool = &state.pool;

    let trades: Vec<TradeRow> =
        sqlx::query_as("SELECT * FROM trades WHERE trade_id > $1::bigint ORDER BY trade_id")
            .bind(cursors.trades)
            .fetch_all(pool)
            .await?;
    for r in trades {
        cursors.trades = cursors.trades.max(r.trade_id as i64);
        let mint = r.mint.clone();
        route_market(state, &mint, BroadcastFrame::Trade(Arc::new(r)), true);
    }

    let swaps: Vec<SwapRow> =
        sqlx::query_as("SELECT * FROM swaps WHERE swap_id > $1::bigint ORDER BY swap_id")
            .bind(cursors.swaps)
            .fetch_all(pool)
            .await?;
    for r in swaps {
        cursors.swaps = cursors.swaps.max(r.swap_id as i64);
        if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
            route_market(state, &mint, BroadcastFrame::Swap(Arc::new(r)), true);
        }
    }

    let messages: Vec<MessageRow> =
        sqlx::query_as("SELECT * FROM messages WHERE message_id > $1::bigint ORDER BY message_id")
            .bind(cursors.messages)
            .fetch_all(pool)
            .await?;
    for r in messages {
        cursors.messages = cursors.messages.max(r.message_id as i64);
        let mint = r.mint.clone();
        route_market(state, &mint, BroadcastFrame::Message(Arc::new(r)), false);
    }

    let pevents: Vec<PositionEventRow> =
        sqlx::query_as("SELECT * FROM position_events WHERE event_id > $1 ORDER BY event_id")
            .bind(cursors.position_events)
            .fetch_all(pool)
            .await?;
    for r in pevents {
        cursors.position_events = cursors.position_events.max(r.event_id);
        let mint = r.mint.clone();
        route_market(state, &mint, BroadcastFrame::PositionEvent(Arc::new(r)), false);
    }

    let reserves: Vec<ReservesRow> =
        sqlx::query_as("SELECT * FROM reserves WHERE reserve_id > $1::bigint ORDER BY reserve_id")
            .bind(cursors.reserves)
            .fetch_all(pool)
            .await?;
    for r in reserves {
        cursors.reserves = cursors.reserves.max(r.reserve_id as i64);
        if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
            route_market(state, &mint, BroadcastFrame::Reserves(Arc::new(r)), false);
        }
    }

    let liq: Vec<LiquidityRow> = sqlx::query_as(
        "SELECT * FROM liquidity_events WHERE liquidity_id > $1::bigint ORDER BY liquidity_id",
    )
    .bind(cursors.liquidity)
    .fetch_all(pool)
    .await?;
    for r in liq {
        cursors.liquidity = cursors.liquidity.max(r.liquidity_id as i64);
        if let Some(mint) = pool_mint(pool, pool_mints, r.pool_id).await? {
            route_market(state, &mint, BroadcastFrame::Liquidity(Arc::new(r)), false);
        }
    }

    Ok(())
}
