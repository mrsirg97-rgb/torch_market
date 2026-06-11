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
        SubscribeTarget::Market { market } => Some(RoomKey::Market(market.clone())),
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rooms: StreamMap<RoomKey, BroadcastStream<Arc<BroadcastFrame>>> = StreamMap::new();
    crate::metrics::METRICS.ws_connections.inc();
    debug!("ws client connected");

    loop {
        tokio::select! {
            biased;

            msg = receiver.next() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(m) = serde_json::from_str::<ClientMsg>(&text) {
                            match m {
                                ClientMsg::Subscribe(t) => {
                                    if let Some(key) = target_key(&t) {
                                        let rx = state.rooms.subscribe(key.clone());
                                        rooms.insert(key, BroadcastStream::new(rx));
                                    }
                                }
                                ClientMsg::Unsubscribe(t) => {
                                    if let Some(key) = target_key(&t) {
                                        rooms.remove(&key);
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
    crate::metrics::METRICS.ws_connections.dec();
    debug!("ws client disconnected");
}
