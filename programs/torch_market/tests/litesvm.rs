// Litesvm integration test suite. See docs/litesvm.md for design.
//
// One test binary; submodules per handler domain. Coverage gate:
// every reachable TorchMarketError variant must appear in at least one
// `expect_err!` call across the suite.

#[path = "litesvm/harness.rs"]
mod harness;

#[path = "litesvm/sanity.rs"]
mod sanity;

#[path = "litesvm/tier_b.rs"]
mod tier_b;

#[path = "litesvm/buy.rs"]
mod buy;

#[path = "litesvm/sell.rs"]
mod sell;

#[path = "litesvm/migration.rs"]
mod migration;

#[path = "litesvm/reclaim.rs"]
mod reclaim;

#[path = "litesvm/revival.rs"]
mod revival;

#[path = "litesvm/lending.rs"]
mod lending;

#[path = "litesvm/short.rs"]
mod short;

#[path = "litesvm/vault.rs"]
mod vault;

#[path = "litesvm/treasury.rs"]
mod treasury;

#[path = "litesvm/protocol_treasury.rs"]
mod protocol_treasury;

#[path = "litesvm/rewards.rs"]
mod rewards;

#[path = "litesvm/coverage.rs"]
mod coverage;

#[path = "litesvm/tx_size.rs"]
mod tx_size;
