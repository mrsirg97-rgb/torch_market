// API-side metrics (the ingest service keeps its own registry). The WS and
// LISTEN gauges left with the rooms (ws-to-indexer); the read tier keeps a
// registry so /metrics stays a valid scrape target.
use once_cell::sync::Lazy;
use prometheus::{Encoder, Registry, TextEncoder};

pub struct Metrics {
    pub registry: Registry,
}

pub static METRICS: Lazy<Metrics> = Lazy::new(|| Metrics {
    registry: Registry::new(),
});

pub fn render() -> String {
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    let _ = encoder.encode(&METRICS.registry.gather(), &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}
