// API service config. DATABASE_URL must be the SELECT-only `torch_api` role —
// this service cannot write the projection, by grant (prompt-003).
use anyhow::Context;

pub struct Config {
    pub database_url: String,
    pub api_bind: String,
    // Optional: enables the /rpc + /rpc-ws proxy (prompt-005).
    pub rpc_url: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL not set")?,
            api_bind: std::env::var("API_BIND").unwrap_or_else(|_| "127.0.0.1:8081".to_string()),
            rpc_url: std::env::var("RPC_URL").ok(),
        })
    }
}
