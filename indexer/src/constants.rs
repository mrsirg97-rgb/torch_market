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
