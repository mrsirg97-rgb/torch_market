// Shared axum state: the read pool (torch_api role, SELECT-only) + rooms.
use sqlx::PgPool;

use crate::ws::Rooms;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub rooms: Rooms,
}
