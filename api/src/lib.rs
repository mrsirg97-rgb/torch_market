// torch read API (prompt-003 split): domain reads + services + axum, plus
// the RPC proxy. The writer lives in /indexer and, since ws-to-indexer, so
// do the WS rooms (/events): the read tier is stateless HTTP over the
// SELECT-only role, nothing else.

pub mod config;
pub mod domain;
pub mod http;
pub mod metrics;
pub mod rpc;
pub mod services;
pub mod state;

pub use torch_indexer_core::{constants, contracts, db, error};
