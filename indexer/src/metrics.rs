// Prometheus metrics. Exposed at GET /metrics in Prometheus text format
// (v0.0.4 exposition).
//
// Single global `METRICS` singleton because the indexer is a single process
// with one writer task and one subscriber task — no need for per-request
// or per-task isolation. Counters are unsigned ints (cheap in the hot
// path); gauges are signed since slot drift can go either way during
// reorgs.
//
// Labels:
//   - `program` ∈ {"torch", "deep_pool"} — to separate the two ingest streams
//   - `kind` ∈ event variant name (snake_case) — to track per-event-type rates
//
// Cardinality: bounded — 12 torch event kinds + 4 deep_pool = 16 unique
// (program, kind) tuples for events_total. Safe for any scrape interval.

use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Opts, Registry, TextEncoder};

pub struct Metrics {
    pub registry: Registry,

    /// Decoded events that reached the writer, by program + event kind.
    /// Incremented in the writer task per event. The label cardinality is
    /// fixed (16 combinations across both programs).
    pub events_total: IntCounterVec,

    /// Decode failures at the inner-instruction level — data shape that
    /// looked like an Anchor self-CPI but didn't match any known
    /// discriminator, or whose borsh decode failed. Labeled by program so
    /// you can tell which program's IDL drifted.
    pub decode_errors_total: IntCounterVec,

    /// Successful per-block writer commits (= one transaction, one slot).
    pub blocks_written_total: IntCounter,

    /// Per-block writer commits that failed — checkpoint did NOT advance,
    /// gRPC subscriber will replay on reconnect. High rate here means the DB
    /// is sick or a schema/code mismatch is rejecting writes.
    pub block_write_errors_total: IntCounter,

    /// Last slot successfully committed. Compare against `solana slots
    /// --url <rpc>` for lag; a healthy indexer trails by <100 slots.
    pub last_processed_slot: IntGauge,

    /// Current number of WS clients subscribed to /events. Useful to
    /// distinguish "no writes because no traffic" from "no writes because
    /// nobody's watching."
    pub broadcast_subscribers: IntGauge,
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new();

        let events_total = IntCounterVec::new(
            Opts::new("indexer_events_total", "Decoded events by program and kind"),
            &["program", "kind"],
        )
        .expect("build events_total");
        let decode_errors_total = IntCounterVec::new(
            Opts::new(
                "indexer_decode_errors_total",
                "Decode failures by program",
            ),
            &["program"],
        )
        .expect("build decode_errors_total");
        let blocks_written_total = IntCounter::new(
            "indexer_blocks_written_total",
            "Per-block writer commits",
        )
        .expect("build blocks_written_total");
        let block_write_errors_total = IntCounter::new(
            "indexer_block_write_errors_total",
            "Per-block writer commit failures",
        )
        .expect("build block_write_errors_total");
        let last_processed_slot = IntGauge::new(
            "indexer_last_processed_slot",
            "Last slot successfully committed",
        )
        .expect("build last_processed_slot");
        let broadcast_subscribers = IntGauge::new(
            "indexer_broadcast_subscribers",
            "Current WS subscribers",
        )
        .expect("build broadcast_subscribers");

        registry
            .register(Box::new(events_total.clone()))
            .expect("register events_total");
        registry
            .register(Box::new(decode_errors_total.clone()))
            .expect("register decode_errors_total");
        registry
            .register(Box::new(blocks_written_total.clone()))
            .expect("register blocks_written_total");
        registry
            .register(Box::new(block_write_errors_total.clone()))
            .expect("register block_write_errors_total");
        registry
            .register(Box::new(last_processed_slot.clone()))
            .expect("register last_processed_slot");
        registry
            .register(Box::new(broadcast_subscribers.clone()))
            .expect("register broadcast_subscribers");

        Self {
            registry,
            events_total,
            decode_errors_total,
            blocks_written_total,
            block_write_errors_total,
            last_processed_slot,
            broadcast_subscribers,
        }
    }

    /// Render the current metric snapshot in Prometheus text format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4096);
        let encoder = TextEncoder::new();
        let _ = encoder.encode(&self.registry.gather(), &mut buf);
        buf
    }
}

pub static METRICS: Lazy<Metrics> = Lazy::new(Metrics::new);

// ───────────── Convenience helpers for the hot paths ────────────────────

/// Map an `AnyEvent` to (program_label, kind_label) for the events_total
/// counter. Keeps the increment site in writer.rs to a single line.
pub fn event_labels(event: &crate::contracts::AnyEvent) -> (&'static str, &'static str) {
    use crate::contracts::{AnyEvent, DeepPoolEvent, TorchEvent};
    match event {
        AnyEvent::Torch(e) => (
            "torch",
            match e {
                TorchEvent::MarketCreated(_) => "market_created",
                TorchEvent::BondingCurveTrade(_) => "bonding_curve_trade",
                TorchEvent::MigratedToDex(_) => "migrated_to_dex",
                TorchEvent::VaultSwapExecuted(_) => "vault_swap_executed",
                TorchEvent::OpenShort(_) => "open_short",
                TorchEvent::CloseShort(_) => "close_short",
                TorchEvent::LiquidateShort(_) => "liquidate_short",
                TorchEvent::OpenLong(_) => "open_long",
                TorchEvent::CloseLong(_) => "close_long",
                TorchEvent::LiquidateLong(_) => "liquidate_long",
                TorchEvent::BondingCompleted(_) => "bonding_completed",
                TorchEvent::TokenReclaimed(_) => "token_reclaimed",
                TorchEvent::RevivalContribution(_) => "revival_contribution",
                TorchEvent::TokenRevived(_) => "token_revived",
            },
        ),
        AnyEvent::DeepPool(e) => (
            "deep_pool",
            match e {
                DeepPoolEvent::PoolCreated(_) => "pool_created",
                DeepPoolEvent::SwapExecuted(_) => "swap_executed",
                DeepPoolEvent::LiquidityAdded(_) => "liquidity_added",
                DeepPoolEvent::LiquidityRemoved(_) => "liquidity_removed",
            },
        ),
    }
}
