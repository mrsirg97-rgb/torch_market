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
    // [prompt-008 I-4] REQUIRED for the `backfill` subcommand (enforced in main):
    // an unbounded full-history walk against the wrong RPC is the foot-gun the
    // genesis guard backstops.
    pub start_slot: Option<u64>,
    // [prompt-008 I-4] Expected cluster for the genesis-hash guard. Defaults to
    // "devnet" (matches rpc_url's default) so the current preview boots
    // unchanged; a mainnet cutover MUST set SOLANA_CLUSTER=mainnet — and if it
    // forgets, the genesis assert fails closed at boot rather than ingesting
    // wrong-chain data.
    pub cluster: String,
    // Resolved from `cluster` via constants::genesis_for_cluster.
    pub expected_genesis: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        // [prompt-008 I-4] Resolve the expected genesis up front so an unknown
        // SOLANA_CLUSTER fails config load (deny by default) rather than booting
        // unguarded.
        let cluster = std::env::var("SOLANA_CLUSTER").unwrap_or_else(|_| "devnet".to_string());
        let expected_genesis = crate::constants::genesis_for_cluster(&cluster)
            .with_context(|| {
                format!("unknown SOLANA_CLUSTER {cluster:?} (expected mainnet|devnet|testnet)")
            })?
            .to_string();
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
            cluster,
            expected_genesis,
        })
    }
}

fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("required env var {key} is not set"))
}
