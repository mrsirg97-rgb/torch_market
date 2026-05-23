// Indexer-local error variants. Anything outside the hot path can use anyhow directly.

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("instruction data too short for event header")]
    TooShort,
    // Inner ix isn't an emit_cpi! event at all (no EVENT_IX_TAG_LE prefix).
    // Could be a regular CPI like deep_pool::create_pool called from torch.
    // Callers should silently skip these — they're not decode failures.
    #[error("not an Anchor emit_cpi event")]
    NotAnEvent,
    #[error("unknown event discriminator")]
    UnknownDiscriminator,
    #[error("borsh did not consume the full payload — layout mismatch")]
    TrailingBytes,
    #[error("borsh deserialization failed: {0}")]
    Borsh(#[from] std::io::Error),
}
