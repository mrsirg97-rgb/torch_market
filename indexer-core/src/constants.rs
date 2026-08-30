// Bounded channel between the gRPC subscriber task and the writer task.
// If the writer falls behind, the subscriber backpressures onto Laserstream
// rather than buffering indefinitely.
pub const BLOCK_CHANNEL_CAPACITY: usize = 128;

// Per-subscriber WS broadcast buffer. If a client falls behind by more than
// this many frames the broadcast channel drops it; the client reconnects and
// gap-fills via the HTTP API.
pub const BROADCAST_CAPACITY: usize = 1024;

// Fixed 8-byte tag prepended to every Anchor `emit_cpi!` self-CPI's instruction
// data. Constant across all Anchor versions; matches `anchor_lang::event::EVENT_IX_TAG_LE`.
// = 0x1d9acb512ea545e4u64.to_le_bytes()
pub const EVENT_IX_TAG_LE: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

// Memo program id, base58. Memo instructions co-resident with a torch ix in
// the same tx are persisted to `messages`; bare memos (no torch ix) are dropped.
pub const MEMO_PROGRAM_ID: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

// [I-2] On-chain interest parameters, mirrored for the position reconcile:
// accrued_interest_stored = prior + interest(prior_debt, Δslots) − interest_paid.
// Mirrors programs/torch_market constants (DEFAULT_INTEREST_RATE_BPS / EPOCH_DURATION_SLOTS).
pub const INTEREST_RATE_BPS: u128 = 150;
pub const EPOCH_DURATION_SLOTS: u128 = 7 * 24 * 60 * 60 * 1000 / 400;

// [prompt-008 I-4] Canonical Solana cluster genesis hashes. At startup the
// indexer fetches the configured RPC's `getGenesisHash` and asserts it matches
// the expected cluster BEFORE any write — so a misconfigured RPC (e.g. the
// devnet-default RPC paired with mainnet program ids, or vice-versa) can't
// silently ingest foreign-chain history. Append-only idempotency keys are
// signatures, so wrong-cluster rows would otherwise look perfectly legitimate.
// Values verified against each cluster's `getGenesisHash` RPC (2026-06-17); the
// guard is also self-checking — a wrong constant fails the indexer closed at
// boot with the actual hash logged, never silent.
pub const MAINNET_GENESIS_HASH: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
pub const DEVNET_GENESIS_HASH: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
pub const TESTNET_GENESIS_HASH: &str = "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY";

// Resolve a SOLANA_CLUSTER name to its expected genesis hash. Unknown cluster →
// None so config resolution fails closed (deny by default).
pub fn genesis_for_cluster(cluster: &str) -> Option<&'static str> {
    match cluster {
        "mainnet" | "mainnet-beta" => Some(MAINNET_GENESIS_HASH),
        "devnet" => Some(DEVNET_GENESIS_HASH),
        "testnet" => Some(TESTNET_GENESIS_HASH),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_for_cluster_resolves_known_and_rejects_unknown() {
        assert_eq!(genesis_for_cluster("mainnet"), Some(MAINNET_GENESIS_HASH));
        assert_eq!(genesis_for_cluster("mainnet-beta"), Some(MAINNET_GENESIS_HASH));
        assert_eq!(genesis_for_cluster("devnet"), Some(DEVNET_GENESIS_HASH));
        assert_eq!(genesis_for_cluster("testnet"), Some(TESTNET_GENESIS_HASH));
        // Unknown / typo'd clusters must NOT resolve — the guard fails closed.
        assert_eq!(genesis_for_cluster("mainet"), None);
        assert_eq!(genesis_for_cluster(""), None);
        assert_eq!(genesis_for_cluster("localnet"), None);
    }
}
