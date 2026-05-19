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

// short.rs requires migrated state — pending Phase 3 (Raydium migrate helper).
// #[path = "litesvm/short.rs"]
// mod short;
