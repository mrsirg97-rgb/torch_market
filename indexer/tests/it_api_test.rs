// Integration tests for the HTTP API.
//
// Uses tower::ServiceExt::oneshot to invoke axum endpoints in-process —
// no actual network bind, no port assignment, no flakiness. Each test
// builds an AppState backed by a fresh test DB, seeds rows via the writer
// or domain layer, hits the endpoint, asserts the JSON response.

mod common;

use common::{fixtures::*, TestDb};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use torch_indexer::api;
use torch_indexer::contracts::{AppState, Broadcaster};
use torch_indexer::stream::writer::write_events_no_checkpoint;

async fn build_app(db: &TestDb) -> axum::Router {
    let state = AppState {
        pool: db.pool.clone(),
        broadcaster: Broadcaster::new(),
    };
    api::router(state)
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("parse json")
}

// ─── healthz ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn healthz_returns_ok() {
    let db = TestDb::new().await;
    let app = build_app(&db).await;
    let resp = app
        .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn metrics_endpoint_renders_prometheus_text() {
    let db = TestDb::new().await;
    // Seed one event so events_total carries a non-zero sample.
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![de(ev_market_created(1, 2), 100, 0)],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("text/plain"), "content-type = {ct}");

    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).expect("metrics body is utf-8");

    // Non-labeled metrics emit a default 0 sample even when untouched, so
    // they're always present in the exposition.
    for name in [
        "indexer_blocks_written_total",
        "indexer_block_write_errors_total",
        "indexer_last_processed_slot",
        "indexer_broadcast_subscribers",
    ] {
        assert!(text.contains(name), "metrics body missing `{name}`:\n{text}");
    }

    // IntCounterVec (labeled) metrics only appear in the exposition once at
    // least one (program, kind) tuple has been observed. The seeded
    // MarketCreated event must have ticked
    // events_total{program=torch, kind=market_created}.
    assert!(
        text.contains("# TYPE indexer_events_total counter"),
        "expected events_total preamble in:\n{text}"
    );
    let market_created_line = text
        .lines()
        .find(|l| l.starts_with("indexer_events_total{") && l.contains("market_created"))
        .unwrap_or_else(|| panic!("no events_total line for market_created:\n{text}"));
    let value: u64 = market_created_line
        .split_whitespace()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    assert!(value >= 1, "events_total{{market_created}} = {value}, expected >= 1");

    // `blocks_written_total` is bumped in write_block_inner — both live
    // and backfill paths increment it.
    let blocks_line = text
        .lines()
        .find(|l| l.starts_with("indexer_blocks_written_total "))
        .unwrap();
    let blocks: u64 = blocks_line.split_whitespace().last().unwrap().parse().unwrap();
    assert!(blocks >= 1, "blocks_written_total = {blocks}, expected >= 1");
}

// ─── /api/markets ────────────────────────────────────────────────────────

#[tokio::test]
async fn list_markets_empty_returns_empty_array() {
    let db = TestDb::new().await;
    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/markets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_markets_returns_seeded_rows() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_market_created(3, 4), 100, 1),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/markets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let mints: Vec<&str> = arr.iter().map(|m| m["mint"].as_str().unwrap()).collect();
    assert!(mints.contains(&pk58(1).as_str()));
    assert!(mints.contains(&pk58(3).as_str()));
}

#[tokio::test]
async fn list_markets_filters_by_status() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_market_created(3, 4), 100, 1),
            de(ev_pool_created(50, 3, 4), 100, 2),
            de(ev_migrated(3, 50), 100, 3),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/markets?status=MIGRATED")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["mint"].as_str().unwrap(), pk58(3));
    assert_eq!(arr[0]["status"].as_str().unwrap(), "MIGRATED");
}

// ─── /api/markets/:mint ──────────────────────────────────────────────────

#[tokio::test]
async fn get_market_returns_404_for_unknown_mint() {
    let db = TestDb::new().await;
    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/markets/{}", pk58(99)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_market_includes_reserves_after_migration() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_pool_created(50, 1, 2), 100, 1),
            de(ev_migrated(1, 50), 100, 2),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/markets/{}", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["market"]["status"].as_str().unwrap(), "MIGRATED");
    assert_eq!(
        json["market"]["deep_pool_pubkey"].as_str().unwrap(),
        pk58(50)
    );
    // PoolCreated phase 3 wrote an initial reserves row → detail surfaces it.
    assert!(json["reserves"].is_object());
}

// ─── /api/trades ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_trades_filters_by_mint() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_market_created(3, 4), 100, 1),
            de(ev_buy_trade(1, 5, 1_000_000_000, 500_000_000_000_000), 100, 2),
            de(ev_buy_trade(3, 5, 2_000_000_000, 1_000_000_000_000_000), 100, 3),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/trades?mint={}", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["mint"].as_str().unwrap(), pk58(1));
    assert_eq!(arr[0]["is_buy"].as_bool().unwrap(), true);
}

// ─── /api/positions (V21) ────────────────────────────────────────────────

#[tokio::test]
async fn list_positions_returns_seeded_short() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_open_short(1, 5, 999_300_000), 100, 1),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/positions?mint={}&side=short", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["owner"].as_str().unwrap(), pk58(5));
    assert_eq!(arr[0]["side"].as_str().unwrap(), "short");
    // short debt = net tokens borrowed; i64 in PG → JSON number.
    assert_eq!(arr[0]["debt_amount"].as_i64().unwrap(), 999_300_000);
}

// ─── /api/liquidations (V21 event log) ───────────────────────────────────

#[tokio::test]
async fn list_liquidations_returns_liquidation_analytics() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_open_short(1, 5, 999_300_000), 100, 1),
        ],
    )
    .await
    .unwrap();
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(ev_liquidate_short(1, 9, 5, 999_300_000, true), 200, 0)],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/liquidations?mint={}", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["kind"].as_str().unwrap(), "liquidate");
    assert_eq!(arr[0]["liquidator"].as_str().unwrap(), pk58(9));
    assert_eq!(arr[0]["twap_ltv"].as_i64().unwrap(), 9200);
}

// ─── /api/candles ────────────────────────────────────────────────────────

#[tokio::test]
async fn candles_rejects_unknown_interval() {
    let db = TestDb::new().await;
    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/candles?mint={}&interval=2m", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn candles_returns_empty_for_mint_with_no_trades() {
    let db = TestDb::new().await;
    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/candles?mint={}&interval=1m", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    assert_eq!(arr.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn candles_aggregates_trades_into_buckets() {
    let db = TestDb::new().await;
    // Seed two trades in the same minute → one bucket.
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000), 100, 1),
            de(ev_buy_trade(1, 3, 500_000_000, 250_000_000_000_000), 100, 2),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    // Wide time window guarantees the fixed_ts() falls inside.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/candles?mint={}&interval=1m&since=2026-01-01T00:00:00Z&before=2026-02-01T00:00:00Z",
                    pk58(1)
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1, "two trades at same fixed_ts → one bucket");
    let bucket = &arr[0];
    // OHLC fields present, volume sums the SOL amounts.
    assert!(bucket["open"].is_number());
    assert!(bucket["close"].is_number());
    assert!(bucket["volume"].is_number());
    let volume = bucket["volume"].as_f64().unwrap();
    // Two buy trades, sol_in 1e9 + 5e8 = 1.5e9. sol_in + sol_out per row.
    assert!((volume - 1_500_000_000.0).abs() < 1.0, "volume = {volume}");
}

// ─── /api/messages ───────────────────────────────────────────────────────

#[tokio::test]
async fn list_messages_returns_attached_memos() {
    let db = TestDb::new().await;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de_with_memo(
                ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000),
                100,
                1,
                "lfg",
            ),
        ],
    )
    .await
    .unwrap();

    let app = build_app(&db).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/messages?mint={}", pk58(1)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["memo_text"].as_str().unwrap(), "lfg");
    assert_eq!(arr[0]["action_kind"].as_str().unwrap(), "buy");
}
