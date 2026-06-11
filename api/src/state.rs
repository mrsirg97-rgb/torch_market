// Shared axum state: the read pool (torch_api role, SELECT-only) + rooms.
use sqlx::PgPool;

use crate::ws::Rooms;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub rooms: Rooms,
    // RPC proxy (prompt-005): upstream JSON-RPC URL with key inline (from
    // Secret Manager in prod). None disables /rpc + /rpc-ws.
    pub rpc_upstream: Option<String>,
    pub http: reqwest::Client,
}
