// Runtime configuration. All env-driven; .env loaded if present.
//
// Two program IDs because this indexer ingests both torch_market and
// deep_pool events into the same Postgres. Both Yellowstone subscriptions
// are filtered on these.

use anyhow::Context;

#[derive(Clone, Debug)]
pub struct Config {
    // Postgres connection string.
    pub database_url: String,
    // Helius Laserstream gRPC endpoint.
    pub laserstream_url: String,
    // Helius API key (x-token header).
    pub laserstream_token: String,
    // torch_market program id, base58.
    pub torch_program_id: String,
    // deep_pool program id, base58.
    pub deep_pool_program_id: String,
    // HTTP + WS bind address.
    pub api_bind: String,
    // Reorg safety margin — on restart, resume from
    // (last_processed_slot - reorg_buffer_slots).
    pub reorg_buffer_slots: u64,
    // Solana JSON-RPC endpoint. Used by the `backfill` subcommand to walk
    // historical signatures via getSignaturesForAddress + getTransaction.
    // Distinct from Laserstream (gRPC).
    pub rpc_url: String,
    // Optional cutoff for backfill; if set, skip any slot < START_SLOT.
    // Useful for fresh devnet envs to avoid ingesting dead history.
    pub start_slot: Option<u64>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            database_url: env_required("DATABASE_URL")?,
            laserstream_url: env_required("LASERSTREAM_URL")?,
            laserstream_token: env_required("LASERSTREAM_TOKEN")?,
            torch_program_id: env_required("TORCH_PROGRAM_ID")?,
            deep_pool_program_id: env_required("DEEP_POOL_PROGRAM_ID")?,
            api_bind: std::env::var("API_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            reorg_buffer_slots: std::env::var("REORG_BUFFER_SLOTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(32),
            rpc_url: std::env::var("RPC_URL")
                .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string()),
            start_slot: std::env::var("START_SLOT").ok().and_then(|s| s.parse().ok()),
        })
    }
}

fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("required env var {key} is not set"))
}
