// Bin entry point. Module code lives in lib.rs so future integration tests
// can import it. This file just wires the runtime (tokio, tracing,
// subcommand dispatch) and delegates everything else.

use anyhow::Context;
use torch_indexer::constants::BLOCK_CHANNEL_CAPACITY;
use torch_indexer::{config, contracts, db, stream::backfill, stream::grpc, stream::writer};
use tokio::sync::mpsc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "torch_indexer=info,sqlx=warn,tower_http=info".into()),
        )
        .with_target(false)
        .init();

    let cfg = config::Config::from_env().context("load config")?;

    // Subcommands. Default and `run` start the live indexer; `backfill` runs
    // a one-shot historical pass and exits. Backfill needs only DATABASE_URL +
    // RPC_URL + program IDs — Laserstream env vars can be unset for that path.
    match std::env::args().nth(1).as_deref() {
        Some("backfill") => return run_backfill(cfg).await,
        Some("run") | None => {}
        Some(other) => {
            anyhow::bail!("unknown subcommand: {other:?}. expected `run` or `backfill`");
        }
    }

    run_live(cfg).await
}

async fn run_backfill(cfg: config::Config) -> anyhow::Result<()> {
    info!("running historical backfill");
    let pool = db::connect(&cfg.database_url)
        .await
        .context("connect to Postgres")?;
    backfill::run(
        cfg.rpc_url,
        cfg.torch_program_id,
        cfg.deep_pool_program_id,
        cfg.start_slot,
        pool,
    )
    .await
}

async fn run_live(cfg: config::Config) -> anyhow::Result<()> {
    info!("starting torch indexer");

    let pool = db::connect(&cfg.database_url)
        .await
        .context("connect to Postgres")?;

    let last = db::last_processed_slot(&pool)
        .await
        .context("read checkpoint")?;
    // If the checkpoint is more than ~1 hour of devnet slots behind current
    // tip (~9000 slots @ 400ms), Helius Laserstream will happily replay the
    // gap at network rate — wasteful, and the indexer will never catch up
    // to live activity. Cap the lag at MAX_RESUME_GAP_SLOTS; beyond that,
    // fall back to streaming from current tip (resume_slot = 0).
    const MAX_RESUME_GAP_SLOTS: u64 = 9_000;
    let current_tip = fetch_current_slot(&cfg.rpc_url).await;
    let resume = match current_tip {
        Some(tip) if last > 0 && tip.saturating_sub(last) > MAX_RESUME_GAP_SLOTS => {
            tracing::warn!(
                last_checkpoint = last,
                current_tip = tip,
                gap_slots = tip - last,
                "checkpoint far behind tip; resuming from current tip"
            );
            0
        }
        _ => last.saturating_sub(cfg.reorg_buffer_slots),
    };
    info!(
        last_checkpoint = last,
        resume_from = resume,
        current_tip = ?current_tip,
        "resuming subscription"
    );

    let (tx, rx) = mpsc::channel::<contracts::BlockBatch>(BLOCK_CHANNEL_CAPACITY);

    // Writer task: drains the channel, writes per-block. Broadcast left this
    // service (prompt-003): pg_notify fires INSIDE each write txn — Postgres
    // delivers on COMMIT to every listening /api instance.
    let writer_handle = {
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = writer::run_writer(pool, rx).await {
                tracing::error!(error = %e, "writer task exited with error");
            }
        })
    };

    // Subscriber task: Laserstream → decoder → channel. Both program streams
    // multiplexed through one subscription; Yellowstone is slot-ordered
    // globally so a single channel preserves ordering.
    let subscriber_handle = {
        let url = cfg.laserstream_url.clone();
        let token = cfg.laserstream_token.clone();
        let torch_program = cfg.torch_program_id.clone();
        let deep_pool_program = cfg.deep_pool_program_id.clone();
        tokio::spawn(async move {
            if let Err(e) = grpc::run_subscriber(
                url,
                token,
                torch_program,
                deep_pool_program,
                resume,
                tx,
            )
            .await
            {
                tracing::error!(error = %e, "subscriber task exited with error");
            }
        })
    };

    // Minimal ops listener: /healthz + /metrics ONLY (reads + WS live in
    // /api). Exists because Cloud Run requires a listening port, and the
    // events_total-vs-rows canary belongs on prod dashboards anyway.
    let ops = {
        let bind = cfg.api_bind.clone();
        tokio::spawn(async move {
            let app = axum::Router::new()
                .route("/healthz", axum::routing::get(|| async { "ok" }))
                .route("/health", axum::routing::get(|| async { "ok" }))
                .route(
                    "/metrics",
                    axum::routing::get(|| async {
                        torch_indexer::metrics::render()
                    }),
                );
            match tokio::net::TcpListener::bind(&bind).await {
                Ok(l) => {
                    info!(bind = %bind, "ops listener up (healthz/metrics)");
                    if let Err(e) = axum::serve(l, app).await {
                        tracing::error!(error = %e, "ops listener exited");
                    }
                }
                Err(e) => tracing::error!(error = %e, %bind, "ops bind failed"),
            }
        })
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown signal received");
        }
        _ = writer_handle => {}
        _ = subscriber_handle => {}
        _ = ops => {}
    }

    info!("shutting down");
    Ok(())
}

// One-shot getSlot via JSON-RPC. Returns None on any failure — the caller
// falls back to the saved checkpoint when this is unknown.
async fn fetch_current_slot(rpc_url: &str) -> Option<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": [{"commitment": "confirmed"}],
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let resp = client.post(rpc_url).json(&body).send().await.ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    v.get("result").and_then(|s| s.as_u64())
}
