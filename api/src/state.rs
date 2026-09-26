// Shared axum state: the read pool (torch_api role, SELECT-only) + the RPC
// proxy upstream. The WS rooms moved to the indexer (ws-to-indexer).
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    // RPC proxy (prompt-005): upstream JSON-RPC URL with key inline (from
    // Secret Manager in prod). None disables /rpc + /rpc-ws.
    pub rpc_upstream: Option<String>,
    pub http: reqwest::Client,
}
