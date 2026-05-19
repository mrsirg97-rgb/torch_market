// Litesvm integration test suite. One test binary; submodules per domain.
// Ported from origin/deep-pool-integration (Raydium-flavored, main branch).
//
// Run with `cargo test -p torch_market --test litesvm`.
// Requires the program SBF binary at target/deploy/torch_market.so —
// run `cargo build-sbf --manifest-path programs/torch_market/Cargo.toml` first.

#[path = "litesvm/harness.rs"]
mod harness;

#[path = "litesvm/sanity.rs"]
mod sanity;

#[path = "litesvm/buy.rs"]
mod buy;

#[path = "litesvm/sell.rs"]
mod sell;

#[path = "litesvm/revival.rs"]
mod revival;

#[path = "litesvm/reclaim.rs"]
mod reclaim;

#[path = "litesvm/rewards.rs"]
mod rewards;

#[path = "litesvm/protocol_treasury.rs"]
mod protocol_treasury;

#[path = "litesvm/vault.rs"]
mod vault;

#[path = "litesvm/migration.rs"]
mod migration;

#[path = "litesvm/short.rs"]
mod short;

#[path = "litesvm/lending.rs"]
mod lending;

#[path = "litesvm/treasury.rs"]
mod treasury;

#[path = "litesvm/coverage.rs"]
mod coverage;

#[path = "litesvm/tier_b.rs"]
mod tier_b;

// tx_size.rs is dpi-only — it builds raw migration txs against DeepPool's
// account list to measure size. Would need a full Raydium-flavored rewrite;
// deferring as it duplicates what scripts/migrate.ts already exercises.
// #[path = "litesvm/tx_size.rs"]
// mod tx_size;
