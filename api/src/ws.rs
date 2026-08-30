// WS rooms (prompt-003). Clients subscribe to rooms; events route once per
// room instead of firehosing every event to every client:
//   - Market(mint): everything for one market — trades, messages, reserves,
//     positions, swaps/liquidity of its pool. The market page's room.
//   - AllMarkets: thin heartbeat for the index page — market-row updates
//     (status flips, reserve/progress changes) + trade ticks. NOT the firehose.
//
// Mechanism: DashMap<RoomKey, broadcast::Sender>. Rooms create lazily on
// first subscriber and GC when the last receiver drops (send() returning
// Err(no receivers) prunes the entry).

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use dashmap::DashMap;
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamMap};
use tracing::debug;

use crate::contracts::{
    LiquidityRow, MarketRow, MessageRow, MigrationRow, PoolRow, PositionEventRow, PositionRow,
    ReservesRow, SwapRow, TradeRow,
};
use crate::state::AppState;

// Same wire format as the pre-split firehose (docs/indexer.md §WS), one frame
// per persisted event, typed by kind — plus Resync, which tells clients the
// API's LISTEN connection had a gap and they should refetch state they care
// about (NOTIFY is not durable; this is the documented resync contract).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BroadcastFrame {
    // Deep_pool
    Pool(Arc<PoolRow>),
    Swap(Arc<SwapRow>),
    Liquidity(Arc<LiquidityRow>),
    Reserves(Arc<ReservesRow>),
    // Torch
    Market(Arc<MarketRow>),
    Trade(Arc<TradeRow>),
    Message(Arc<MessageRow>),
    Position(Arc<PositionRow>),
    PositionEvent(Arc<PositionEventRow>),
    Migration(Arc<MigrationRow>),
    // Control
    Resync,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoomKey {
    Market(String),
    AllMarkets,
}

const ROOM_CAPACITY: usize = 1024;

// [prompt-008 A-1] Cap rooms a single connection can open. WS frames over an
// established connection are invisible to the edge rate-limiter (one WS upgrade
// counts as one edge request), so an unbounded `subscribe` flood would grow the
// shared room map without limit. The real client (one WS per TorchFeedClient)
// only ever holds `all` + the current market page's room — ~2 live, ~3 during a
// nav transition. 8 leaves headroom for that overlap while still walling a flood.
const MAX_ROOMS_PER_CONN: usize = 8;

// Cheap plausibility check for a market pubkey — a 32-byte key base58-encodes to
// 32-44 chars over the base58 alphabet (no 0 O I l). Rejects the junk-string
// flood without a per-message DB lookup (or the TOCTOU race that would imply).
fn is_plausible_pubkey(s: &str) -> bool {
    (32..=44).contains(&s.len())
        && s.bytes().all(|b| {
            matches!(b,
                b'1'..=b'9' | b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z' | b'a'..=b'k' | b'm'..=b'z')
        })
}

#[derive(Clone, Default)]
pub struct Rooms {
    inner: Arc<DashMap<RoomKey, broadcast::Sender<Arc<BroadcastFrame>>>>,
}

impl Rooms {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, key: RoomKey) -> broadcast::Receiver<Arc<BroadcastFrame>> {
        self.inner
            .entry(key)
            .or_insert_with(|| broadcast::channel(ROOM_CAPACITY).0)
            .subscribe()
    }

    // Publish to a room if it has subscribers; GC the room when the last
    // receiver is gone. No subscribers = no work — events for idle markets
    // cost one DashMap lookup.
    pub fn publish(&self, key: &RoomKey, frame: Arc<BroadcastFrame>) {
        if let Some(sender) = self.inner.get(key) {
            if sender.send(frame).is_err() {
                drop(sender);
                self.inner
                    .remove_if(key, |_, s| s.receiver_count() == 0);
            }
        }
    }

    // Resync control frame to every live room (LISTEN gap recovery).
    pub fn publish_resync_all(&self) {
        for entry in self.inner.iter() {
            let _ = entry.value().send(Arc::new(BroadcastFrame::Resync));
        }
    }

    pub fn active_rooms(&self) -> usize {
        self.inner.len()
    }

    // [prompt-008 A-1] Drop a room when its last receiver goes away. The publish
    // path only prunes on a send to an empty room, which never fires for a room
    // that receives no events (e.g. a subscribed-but-nonexistent mint) — so a
    // connection releases its rooms explicitly on unsubscribe / disconnect.
    pub fn release(&self, key: &RoomKey) {
        self.inner.remove_if(key, |_, s| s.receiver_count() == 0);
    }
}

// Client → server messages. A connection may join any number of rooms.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ClientMsg {
    Subscribe(SubscribeTarget),
    Unsubscribe(SubscribeTarget),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SubscribeTarget {
    All(String), // the literal "all"
    Market { market: String },
}

fn target_key(t: &SubscribeTarget) -> Option<RoomKey> {
    match t {
        SubscribeTarget::All(s) if s == "all" => Some(RoomKey::AllMarkets),
        SubscribeTarget::All(_) => None,
        // [prompt-008 A-1] Only a plausibly-real mint may open a room.
        SubscribeTarget::Market { market } if is_plausible_pubkey(market) => {
            Some(RoomKey::Market(market.clone()))
        }
        SubscribeTarget::Market { .. } => None,
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rooms: StreamMap<RoomKey, BroadcastStream<Arc<BroadcastFrame>>> = StreamMap::new();
    // [prompt-008 A-1] This connection's room keys — drives the per-conn cap and
    // the on-disconnect room release.
    let mut subscribed: HashSet<RoomKey> = HashSet::new();
    crate::metrics::METRICS.ws_connections.inc();
    debug!("ws client connected");

    loop {
        tokio::select! {
            biased;

            msg = receiver.next() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Err(e) => debug!(%text, error = %e, "unparseable client msg"),
                            Ok(m) => {
                        if true {
                            let m = m;
                            match m {
                                ClientMsg::Subscribe(t) => {
                                    if let Some(key) = target_key(&t) {
                                        // [prompt-008 A-1] Cap per-connection rooms;
                                        // insert() guards against re-subscribe churn.
                                        if subscribed.len() >= MAX_ROOMS_PER_CONN
                                            && !subscribed.contains(&key)
                                        {
                                            debug!("subscribe rejected: per-connection room cap");
                                        } else if subscribed.insert(key.clone()) {
                                            debug!(?key, "client subscribed");
                                            let rx = state.rooms.subscribe(key.clone());
                                            rooms.insert(key, BroadcastStream::new(rx));
                                        }
                                    } else {
                                        debug!(?t, "subscribe target rejected");
                                    }
                                }
                                ClientMsg::Unsubscribe(t) => {
                                    if let Some(key) = target_key(&t) {
                                        rooms.remove(&key);
                                        subscribed.remove(&key);
                                        // [prompt-008 A-1] Release the shared room if
                                        // this was its last receiver.
                                        state.rooms.release(&key);
                                    }
                                }
                            }
                        }
                            }
                        }
                    }
                    _ => {}
                }
            }

            frame = rooms.next(), if !rooms.is_empty() => {
                match frame {
                    Some((_key, Ok(f))) => {
                        let json = serde_json::to_string(&*f).unwrap_or_default();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    // Lagged: this client missed frames in that room — tell it
                    // to resync rather than silently dropping the gap.
                    Some((_key, Err(_lag))) => {
                        let json = serde_json::to_string(&BroadcastFrame::Resync).unwrap_or_default();
                        if sender.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {}
                }
            }
        }
    }
    // [prompt-008 A-1] Drop this connection's receivers, then release any rooms
    // that now have none — so idle/junk rooms don't linger in the shared map (the
    // publish-path GC only fires for rooms that actually receive events).
    drop(rooms);
    for key in subscribed.drain() {
        state.rooms.release(&key);
    }
    crate::metrics::METRICS.ws_connections.dec();
    debug!("ws client disconnected");
}

#[cfg(test)]
mod tests {
    use super::is_plausible_pubkey;

    #[test]
    fn plausible_pubkey_accepts_real_keys_rejects_junk() {
        // Real 32-byte pubkeys (base58, 43-44 chars).
        assert!(is_plausible_pubkey(
            "So11111111111111111111111111111111111111112"
        ));
        assert!(is_plausible_pubkey(
            "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG"
        ));
        // All-zero pubkey → 32 '1's, the lower length bound.
        assert!(is_plausible_pubkey("11111111111111111111111111111111"));

        // Junk-flood inputs the room map must refuse.
        assert!(!is_plausible_pubkey(""), "empty");
        assert!(!is_plausible_pubkey("all"), "too short");
        assert!(!is_plausible_pubkey(&"a".repeat(45)), "too long");
        // Non-base58 chars (0, O, I, l) are out of the alphabet.
        assert!(!is_plausible_pubkey(
            "0OIl1111111111111111111111111111111111111111"
        ));
    }
}
