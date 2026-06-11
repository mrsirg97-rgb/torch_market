// API-side metrics (the ingest service keeps its own registry).
use once_cell::sync::Lazy;
use prometheus::{IntCounter, IntGauge, Registry};

pub struct Metrics {
    pub registry: Registry,
    pub ws_connections: IntGauge,
    pub rooms_active: IntGauge,
    pub notifies_received_total: IntCounter,
    pub listen_resyncs_total: IntCounter,
}

pub static METRICS: Lazy<Metrics> = Lazy::new(|| {
    let registry = Registry::new();
    let ws_connections =
        IntGauge::new("api_ws_connections", "Open WebSocket connections").unwrap();
    let rooms_active = IntGauge::new("api_rooms_active", "Live broadcast rooms").unwrap();
    let notifies_received_total =
        IntCounter::new("api_notifies_received_total", "pg_notify payloads received").unwrap();
    let listen_resyncs_total = IntCounter::new(
        "api_listen_resyncs_total",
        "LISTEN reconnects that triggered a gap replay + Resync broadcast",
    )
    .unwrap();
    registry.register(Box::new(ws_connections.clone())).unwrap();
    registry.register(Box::new(rooms_active.clone())).unwrap();
    registry
        .register(Box::new(notifies_received_total.clone()))
        .unwrap();
    registry
        .register(Box::new(listen_resyncs_total.clone()))
        .unwrap();
    Metrics {
        registry,
        ws_connections,
        rooms_active,
        notifies_received_total,
        listen_resyncs_total,
    }
});

pub fn render() -> String {
    use prometheus::Encoder;
    let mut buf = Vec::new();
    let _ = prometheus::TextEncoder::new().encode(&METRICS.registry.gather(), &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}
