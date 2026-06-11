// WRITE-side domain (prompt-003): insert/upsert/apply mutations called by
// the single-writer ingest. Read-level queries live in /api. The two reads
// kept here (position::get, pool::list) are atomic-write internals — the
// reconcile and the pool-id cache run INSIDE the write transaction.

pub mod market;
pub mod message;
pub mod migration;
pub mod position;
pub mod trade;

pub mod liquidity;
pub mod pool;
pub mod reserves;
pub mod swap;

pub use pool::PoolFilter;
