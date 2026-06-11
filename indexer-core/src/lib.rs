// Shared core for the torch indexer split (prompt-003):
//   /indexer (ingest: stream → writer → pg_notify, single instance)
//   /api     (reads: domain queries, services, axum, WS rooms, LISTEN)
// This crate holds only what BOTH sides speak: row/event/enum contracts,
// the Postgres pool initializer, error types, constants.

pub mod constants;
pub mod contracts;
pub mod db;
pub mod error;
