// torch read-API service (prompt-003): stateless axum over the SELECT-only
// role. Scales horizontally with nothing to coordinate — the WS rooms live
// with the single writer (ws-to-indexer), so no instance holds a LISTEN
// connection or any per-connection state.
use anyhow::Context;
use torch_api::{config, db, http, state::AppState};
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
        rpc_upstream: cfg.rpc_url.clone(),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("http client"),
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
        _ = server_handle => {}
    }
    Ok(())
}
