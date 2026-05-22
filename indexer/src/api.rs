// HTTP + WS API.
//
// Endpoints per docs/indexer.md §"API surface":
//   GET  /healthz                  liveness
//   GET  /api/markets              list (status/creator/tier/since/limit filters)
//   GET  /api/markets/:mint        detail (market row + current reserves if migrated)
//   GET  /api/trades               (mint/trader/since/before/limit)
//   GET  /api/messages             (mint/sender/since/before/limit)
//   GET  /api/loans                (mint/health/is_active)
//   GET  /api/shorts               (mint/health/is_active)
//   GET  /api/migrations           (mint/since/before)
//   GET  /api/pools                (deep_pool — token_mint/creator/limit)
//   GET  /api/pools/:pubkey        deep_pool detail
//   GET  /api/swaps                (deep_pool — pool_id/user/token_mint/since/before/limit)
//   GET  /api/liquidity            (deep_pool — pool_id/provider/token_mint/is_add/since/before)
//   GET  /api/candles              OHLCV bucketed; joins trades ∪ swaps for a mint
//   WS   /events                   firehose, post-commit
//
// All HTTP handlers open a per-request REPEATABLE READ Postgres transaction
// (via RequestCtx::begin), call services, then commit. Errors auto-rollback
// via sqlx's Transaction Drop impl; success commits explicitly.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, warn};

use crate::contracts::{
    AppState, LiquidityRow, LoanRow, MarketRow, MarketStatus, MarketTier, MessageRow,
    MigrationRow, PoolRow, PositionHealth, ReservesRow, ShortRow, SwapRow, TradeRow,
};
use crate::domain::{
    LiquidityFilter, LoanFilter, MarketFilter, MessageFilter, MigrationFilter, PoolFilter,
    ShortFilter, SwapFilter, TradeFilter,
};
use crate::services::RequestCtx;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        // Torch endpoints
        .route("/api/markets", get(list_markets))
        .route("/api/markets/:mint", get(get_market))
        .route("/api/trades", get(list_trades))
        .route("/api/messages", get(list_messages))
        .route("/api/loans", get(list_loans))
        .route("/api/shorts", get(list_shorts))
        .route("/api/migrations", get(list_migrations))
        .route("/api/candles", get(list_candles))
        // deep_pool endpoints
        .route("/api/pools", get(list_pools))
        .route("/api/pools/:pubkey", get(get_pool))
        .route("/api/swaps", get(list_swaps))
        .route("/api/liquidity", get(list_liquidity))
        // WS firehose
        .route("/events", get(ws_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

// Prometheus text-format (v0.0.4). Scrape with the standard exposition
// `Content-Type` header so Prometheus parses inline rather than falling
// back to OpenMetrics auto-detection.
async fn metrics() -> impl IntoResponse {
    let body = crate::metrics::METRICS.encode();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

// ---------- /api/markets ----------

#[derive(Debug, Deserialize)]
struct MarketsQuery {
    status: Option<MarketStatus>,
    tier: Option<MarketTier>,
    creator: Option<String>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_markets(
    State(state): State<AppState>,
    Query(q): Query<MarketsQuery>,
) -> Result<Json<Vec<MarketRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let markets = ctx
        .markets()
        .list(MarketFilter {
            status: q.status,
            tier: q.tier,
            creator: q.creator,
            since: q.since,
            before: q.before,
            limit: Some(limit),
            ..Default::default()
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(markets)))
}

// ---------- /api/markets/:mint ----------

#[derive(Debug, Serialize)]
struct MarketDetail {
    market: MarketRow,
    // Current DEX reserves if migrated; None otherwise.
    reserves: Option<ReservesRow>,
}

async fn get_market(
    State(state): State<AppState>,
    Path(mint): Path<String>,
) -> Result<Json<MarketDetail>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let market = ctx
        .markets()
        .by_mint(&mint)
        .await?
        .ok_or(ApiError::NotFound)?;

    let reserves = if let Some(pubkey) = &market.deep_pool_pubkey {
        let pool = ctx.pools().by_pubkey(pubkey).await?;
        if let Some(p) = pool {
            ctx.reserves().latest_for_pool(p.pool_id).await?
        } else {
            None
        }
    } else {
        None
    };

    ctx.commit().await?;
    Ok(Json(MarketDetail {
        market: (*market).clone(),
        reserves: reserves.map(|r| (*r).clone()),
    }))
}

// ---------- /api/trades ----------

#[derive(Debug, Deserialize)]
struct TradesQuery {
    mint: Option<String>,
    trader: Option<String>,
    is_buy: Option<bool>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_trades(
    State(state): State<AppState>,
    Query(q): Query<TradesQuery>,
) -> Result<Json<Vec<TradeRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let trades = ctx
        .trades()
        .list(TradeFilter {
            mint: q.mint,
            trader: q.trader,
            is_buy: q.is_buy,
            since: q.since,
            before: q.before,
            limit: Some(limit),
            ..Default::default()
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(trades)))
}

// ---------- /api/messages ----------

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    mint: Option<String>,
    sender: Option<String>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_messages(
    State(state): State<AppState>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<Vec<MessageRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let messages = ctx
        .messages()
        .list(MessageFilter {
            mint: q.mint,
            sender: q.sender,
            since: q.since,
            before: q.before,
            limit: Some(limit),
            ..Default::default()
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(messages)))
}

// ---------- /api/loans ----------

#[derive(Debug, Deserialize)]
struct LoansQuery {
    mint: Option<String>,
    borrower: Option<String>,
    health: Option<PositionHealth>,
    is_active: Option<bool>,
    limit: Option<i64>,
}

async fn list_loans(
    State(state): State<AppState>,
    Query(q): Query<LoansQuery>,
) -> Result<Json<Vec<LoanRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let loans = ctx
        .loans()
        .list(LoanFilter {
            mint: q.mint,
            borrower: q.borrower,
            health: q.health,
            is_active: q.is_active,
            limit: Some(limit),
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(loans)))
}

// ---------- /api/shorts ----------

#[derive(Debug, Deserialize)]
struct ShortsQuery {
    mint: Option<String>,
    shorter: Option<String>,
    health: Option<PositionHealth>,
    is_active: Option<bool>,
    limit: Option<i64>,
}

async fn list_shorts(
    State(state): State<AppState>,
    Query(q): Query<ShortsQuery>,
) -> Result<Json<Vec<ShortRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let shorts = ctx
        .shorts()
        .list(ShortFilter {
            mint: q.mint,
            shorter: q.shorter,
            health: q.health,
            is_active: q.is_active,
            limit: Some(limit),
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(shorts)))
}

// ---------- /api/migrations ----------

#[derive(Debug, Deserialize)]
struct MigrationsQuery {
    mint: Option<String>,
    deep_pool_pubkey: Option<String>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_migrations(
    State(state): State<AppState>,
    Query(q): Query<MigrationsQuery>,
) -> Result<Json<Vec<MigrationRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let migrations = ctx
        .migrations()
        .list(MigrationFilter {
            mint: q.mint,
            deep_pool_pubkey: q.deep_pool_pubkey,
            since: q.since,
            before: q.before,
            limit: Some(limit),
        })
        .await?;
    ctx.commit().await?;
    Ok(Json(arc_owned(migrations)))
}

// ---------- /api/candles ----------
//
// OHLCV across the full lifecycle of a mint: bonding-curve trades (pre-
// migration) UNION post-migration deep_pool swaps. Bucketed by
// `floor(epoch / interval_seconds) * interval_seconds`, which works in
// vanilla Postgres without TimescaleDB.

#[derive(Debug, Deserialize)]
struct CandlesQuery {
    mint: String,
    interval: String, // 1m | 5m | 1h
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Candle {
    bucket_start: DateTime<Utc>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    close: Option<f64>,
    volume: Option<f64>,
}

async fn list_candles(
    State(state): State<AppState>,
    Query(q): Query<CandlesQuery>,
) -> Result<Json<Vec<Candle>>, ApiError> {
    let bucket_seconds = match q.interval.as_str() {
        "1m" => 60,
        "5m" => 300,
        "1h" => 3600,
        _ => return Err(ApiError::BadRequest("interval must be 1m | 5m | 1h")),
    };

    // Default window: last 24h if neither bound given.
    let now = Utc::now();
    let since = q.since.unwrap_or_else(|| now - chrono::Duration::hours(24));
    let before = q.before.unwrap_or(now);

    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let rows: Vec<Candle> = sqlx::query_as(
        "WITH all_events AS (
            SELECT
                created_at,
                CASE WHEN tokens_out > 0
                     THEN sol_in::float8 / tokens_out
                     WHEN tokens_in > 0
                     THEN sol_out::float8 / tokens_in
                     ELSE NULL END AS price,
                (sol_in + sol_out)::float8 AS volume_sol
            FROM trades
            WHERE mint = $1
              AND created_at >= $2 AND created_at < $3
            UNION ALL
            SELECT
                s.created_at,
                CASE WHEN s.is_buy AND s.amount_out_net > 0
                     THEN s.amount_in_net::float8 / s.amount_out_net
                     WHEN NOT s.is_buy AND s.amount_in_net > 0
                     THEN s.amount_out_net::float8 / s.amount_in_net
                     ELSE NULL END AS price,
                (CASE WHEN s.is_buy
                      THEN s.amount_in_net
                      ELSE s.amount_out_net END)::float8 AS volume_sol
            FROM swaps s
            JOIN pools p ON s.pool_id = p.pool_id
            WHERE p.token_mint = $1
              AND s.created_at >= $2 AND s.created_at < $3
         )
         SELECT
             -- `to_timestamp` returns `timestamptz` (UTC by construction since
             -- the epoch is timezone-agnostic). Do NOT add `AT TIME ZONE 'UTC'`
             -- here — that strips the tz and yields a plain `timestamp` which
             -- sqlx refuses to decode into DateTime<Utc>.
             to_timestamp(
                 floor(extract(epoch FROM created_at) / $4::float8) * $4::float8
             ) AS bucket_start,
             (array_agg(price ORDER BY created_at ASC) FILTER (WHERE price IS NOT NULL))[1] AS open,
             max(price) AS high,
             min(price) AS low,
             (array_agg(price ORDER BY created_at DESC) FILTER (WHERE price IS NOT NULL))[1] AS close,
             sum(volume_sol) AS volume
         FROM all_events
         GROUP BY bucket_start
         ORDER BY bucket_start ASC",
    )
    .bind(&q.mint)
    .bind(since)
    .bind(before)
    .bind(bucket_seconds as f64)
    .fetch_all(&mut *ctx.tx)
    .await?;
    ctx.commit().await?;

    Ok(Json(rows))
}

// ---------- /api/pools (deep_pool) ----------

#[derive(Debug, Deserialize)]
struct PoolsQuery {
    token_mint: Option<String>,
    creator: Option<String>,
    limit: Option<i64>,
}

async fn list_pools(
    State(state): State<AppState>,
    Query(q): Query<PoolsQuery>,
) -> Result<Json<Vec<PoolRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let pools = if let Some(mint) = q.token_mint {
        ctx.pools().for_token_mints(vec![mint]).await?
    } else if let Some(creator) = q.creator {
        ctx.pools().for_creators(vec![creator]).await?
    } else {
        ctx.pools()
            .list(PoolFilter {
                limit: q.limit,
                ..Default::default()
            })
            .await?
    };
    ctx.commit().await?;
    Ok(Json(arc_owned(pools)))
}

#[derive(Debug, Serialize)]
struct PoolDetail {
    pool: PoolRow,
    reserves: Option<ReservesRow>,
}

async fn get_pool(
    State(state): State<AppState>,
    Path(pubkey): Path<String>,
) -> Result<Json<PoolDetail>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let pool = ctx
        .pools()
        .by_pubkey(&pubkey)
        .await?
        .ok_or(ApiError::NotFound)?;
    let reserves = ctx.reserves().latest_for_pool(pool.pool_id).await?;
    ctx.commit().await?;
    Ok(Json(PoolDetail {
        pool: (*pool).clone(),
        reserves: reserves.map(|r| (*r).clone()),
    }))
}

// ---------- /api/swaps (deep_pool) ----------

#[derive(Debug, Deserialize)]
struct SwapsQuery {
    pool_id: Option<i32>,
    user: Option<String>,
    token_mint: Option<String>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_swaps(
    State(state): State<AppState>,
    Query(q): Query<SwapsQuery>,
) -> Result<Json<Vec<SwapRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let swaps = if let Some(mint) = q.token_mint {
        ctx.swaps().for_token_mint(&mint, Some(limit)).await?
    } else {
        ctx.swaps()
            .list(SwapFilter {
                pool_ids: q.pool_id.map(|id| vec![id]),
                users: q.user.map(|u| vec![u]),
                since: q.since,
                before: q.before,
                limit: Some(limit),
                ..Default::default()
            })
            .await?
    };
    ctx.commit().await?;
    Ok(Json(arc_owned(swaps)))
}

// ---------- /api/liquidity (deep_pool) ----------

#[derive(Debug, Deserialize)]
struct LiquidityQuery {
    pool_id: Option<i32>,
    provider: Option<String>,
    token_mint: Option<String>,
    is_add: Option<bool>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

async fn list_liquidity(
    State(state): State<AppState>,
    Query(q): Query<LiquidityQuery>,
) -> Result<Json<Vec<LiquidityRow>>, ApiError> {
    let mut ctx = RequestCtx::begin(&state.pool).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let events = if let Some(mint) = q.token_mint {
        ctx.liquidity().for_token_mint(&mint, Some(limit)).await?
    } else {
        ctx.liquidity()
            .list(LiquidityFilter {
                pool_ids: q.pool_id.map(|id| vec![id]),
                providers: q.provider.map(|p| vec![p]),
                is_add: q.is_add,
                since: q.since,
                before: q.before,
                limit: Some(limit),
                ..Default::default()
            })
            .await?
    };
    ctx.commit().await?;
    Ok(Json(arc_owned(events)))
}

// ---------- WS /events ----------

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcaster.subscribe();
    debug!(
        subscribers = state.broadcaster.subscriber_count(),
        "ws client connected"
    );

    loop {
        tokio::select! {
            biased;

            msg = receiver.next() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    _ => {}
                }
            }

            frame = rx.recv() => {
                match frame {
                    Ok(f) => {
                        let json = serde_json::to_string(&f).unwrap_or_default();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!(skipped = n, "ws subscriber lagged");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
    debug!("ws client disconnected");
}

// ---------- helpers ----------

fn arc_owned<T: Clone>(arcs: Vec<Arc<T>>) -> Vec<T> {
    arcs.into_iter().map(|a| (*a).clone()).collect()
}

#[derive(Debug)]
enum ApiError {
    Db(sqlx::Error),
    NotFound,
    BadRequest(&'static str),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Db(e) => {
                tracing::error!(error = %e, "db error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
        }
    }
}
