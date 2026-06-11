// torch read-API service (prompt-003): axum + WS rooms, fed by Postgres
// LISTEN. Scales horizontally — every instance holds its own LISTEN
// connection and its own rooms; the single-writer ingest never knows we exist.
use anyhow::Context;
use torch_api::{config, db, http, listen, state::AppState, ws::Rooms};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "torch_api=info,sqlx=warn,tower_http=info".into()),
        )
        .with_target(false)
        .init();

    let cfg = config::Config::from_env().context("load config")?;
    let pool = db::connect(&cfg.database_url)
        .await
        .context("connect to Postgres")?;

    let state = AppState {
        pool: pool.clone(),
        rooms: Rooms::new(),
    };

    // LISTEN bridge: pg_notify → rooms.
    let listener_handle = {
        let state = state.clone();
        let url = cfg.database_url.clone();
        tokio::spawn(async move {
            if let Err(e) = listen::run_listener(state, url).await {
                tracing::error!(error = %e, "listener task exited");
            }
        })
    };

    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.api_bind)
        .await
        .with_context(|| format!("bind {}", cfg.api_bind))?;
    info!(bind = %cfg.api_bind, "api listening");

    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "server exited");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => { info!("shutdown signal"); }
        _ = listener_handle => {}
        _ = server_handle => {}
    }
    Ok(())
}
