// Indexer-local error variants. Anything outside the hot path can use anyhow directly.

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("instruction data too short for event header")]
    TooShort,
    #[error("unknown event discriminator")]
    UnknownDiscriminator,
    #[error("borsh did not consume the full payload — layout mismatch")]
    TrailingBytes,
    #[error("borsh deserialization failed: {0}")]
    Borsh(#[from] std::io::Error),
}
