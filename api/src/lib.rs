// torch read API (prompt-003 split): domain reads + services + axum + WS
// rooms fed by Postgres LISTEN. The writer lives in /indexer; this crate
// holds no write path and its DB role has no write grants.

pub mod config;
pub mod domain;
pub mod http;
pub mod listen;
pub mod metrics;
pub mod rpc;
pub mod services;
pub mod state;
pub mod ws;

pub use torch_indexer_core::{constants, contracts, db, error};
