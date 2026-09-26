// The ws-to-indexer contract: the writer is the broadcaster.
//   1. a committed block's rows reach the rooms post-COMMIT — the row is
//      readable by the time its frame arrives — in commit order
//   2. the market room sees everything for its mint; AllMarkets sees only
//      what the index page sorts on (market rows, trade ticks)
//   3. a block that fails to write publishes nothing: no frame without a row,
//      and the writer survives to publish the next good block
//
// Drives the real live path (`run_writer` over an mpsc channel) against a
// fresh DB, exactly as the gRPC subscriber feeds it in production.

mod common;

use common::{fixtures::*, TestDb};

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use torch_api::contracts::BlockBatch;
use torch_api::domain::{market, trade, TradeFilter};
use torch_indexer::stream::writer::run_writer;
use torch_indexer::ws::{BroadcastFrame, RoomKey, Rooms};

type Rx = broadcast::Receiver<Arc<BroadcastFrame>>;

async fn next_frame(rx: &mut Rx) -> Arc<BroadcastFrame> {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("frame within 2s")
        .expect("room open")
}

async fn assert_silent(rx: &mut Rx) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
        "room must be silent"
    );
}

fn batch(slot: u64, events: Vec<torch_api::contracts::DecodedEvent>) -> BlockBatch {
    BlockBatch { slot, events }
}

async fn checkpoint(db: &TestDb) -> Option<i64> {
    sqlx::query_scalar("SELECT last_processed_slot FROM indexer_state WHERE id = 1")
        .fetch_optional(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn committed_rows_reach_rooms_post_commit_in_commit_order() {
    let db = TestDb::new().await;
    let rooms = Rooms::new();
    let mut market_rx = rooms.subscribe(RoomKey::Market(pk58(1)));
    let mut all_rx = rooms.subscribe(RoomKey::AllMarkets);

    let (tx, rx) = mpsc::channel::<BlockBatch>(8);
    let writer = tokio::spawn(run_writer(db.pool.clone(), rx, rooms.clone()));

    tx.send(batch(
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(
                ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000),
                100,
                1,
            ),
        ],
    ))
    .await
    .unwrap();

    // Market room: the market row, then its trade — publish order is the
    // writer's phase order (markets → trades), i.e. commit order.
    let f = next_frame(&mut market_rx).await;
    let BroadcastFrame::Market(m) = &*f else {
        panic!("first frame must be the market row, got {f:?}")
    };
    assert_eq!(m.mint, pk58(1));
    // Post-COMMIT: the row is already readable on an independent connection.
    let mut t = db.pool.begin().await.unwrap();
    assert!(
        market::get_by_mint(&mut t, &pk58(1))
            .await
            .unwrap()
            .is_some(),
        "frame arrived before its row committed"
    );

    let f = next_frame(&mut market_rx).await;
    let BroadcastFrame::Trade(tr) = &*f else {
        panic!("second frame must be the trade, got {f:?}")
    };
    assert_eq!(tr.mint, pk58(1));
    assert_eq!(tr.slot, 100);
    assert!(tr.is_buy);
    let trades = trade::list(
        &mut t,
        TradeFilter {
            mint: Some(pk58(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        trades.len(),
        1,
        "trade frame arrived before its row committed"
    );
    drop(t);

    // AllMarkets heartbeat: market row + trade tick, same order.
    assert!(matches!(
        *next_frame(&mut all_rx).await,
        BroadcastFrame::Market(_)
    ));
    assert!(matches!(
        *next_frame(&mut all_rx).await,
        BroadcastFrame::Trade(_)
    ));

    // A later block's frames come strictly after — slot order is commit order.
    tx.send(batch(
        101,
        vec![de(
            ev_buy_trade(1, 4, 2_000_000_000, 400_000_000_000_000),
            101,
            0,
        )],
    ))
    .await
    .unwrap();
    let f = next_frame(&mut market_rx).await;
    let BroadcastFrame::Trade(tr) = &*f else {
        panic!("expected the slot-101 trade, got {f:?}")
    };
    assert_eq!(tr.slot, 101);
    assert!(matches!(
        *next_frame(&mut all_rx).await,
        BroadcastFrame::Trade(_)
    ));
    assert_eq!(checkpoint(&db).await, Some(101));

    // Nothing else leaked into either room.
    assert_silent(&mut market_rx).await;
    assert_silent(&mut all_rx).await;

    drop(tx);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn market_room_is_isolated_and_all_markets_is_only_the_heartbeat() {
    let db = TestDb::new().await;
    let rooms = Rooms::new();
    let mut room_1 = rooms.subscribe(RoomKey::Market(pk58(1)));
    let mut room_2 = rooms.subscribe(RoomKey::Market(pk58(2)));
    let mut all_rx = rooms.subscribe(RoomKey::AllMarkets);

    let (tx, rx) = mpsc::channel::<BlockBatch>(8);
    let writer = tokio::spawn(run_writer(db.pool.clone(), rx, rooms.clone()));

    // Market 1 gets a market row and a trade; market 2 gets a market row and a
    // message (memo on a trade) — messages are market-page only, never on the
    // index-page heartbeat.
    tx.send(batch(
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_market_created(2, 2), 100, 1),
            de(
                ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000),
                100,
                2,
            ),
            de_with_memo(
                ev_buy_trade(2, 3, 1_000_000_000, 500_000_000_000_000),
                100,
                3,
                "hello",
            ),
        ],
    ))
    .await
    .unwrap();

    // Room 1: exactly its market + its trade.
    assert!(matches!(
        *next_frame(&mut room_1).await,
        BroadcastFrame::Market(_)
    ));
    let f = next_frame(&mut room_1).await;
    let BroadcastFrame::Trade(tr) = &*f else {
        panic!("got {f:?}")
    };
    assert_eq!(tr.mint, pk58(1));
    assert_silent(&mut room_1).await;

    // Room 2: its market, its trade, and its message — nothing from market 1.
    let mut kinds = Vec::new();
    for _ in 0..3 {
        let f = next_frame(&mut room_2).await;
        let mint = match &*f {
            BroadcastFrame::Market(m) => m.mint.clone(),
            BroadcastFrame::Trade(t) => t.mint.clone(),
            BroadcastFrame::Message(m) => m.mint.clone(),
            other => panic!("unexpected frame in room 2: {other:?}"),
        };
        assert_eq!(mint, pk58(2), "cross-room leak");
        kinds.push(std::mem::discriminant(&*f));
    }
    assert_eq!(kinds.len(), 3);
    assert_silent(&mut room_2).await;

    // AllMarkets: 2 market rows + 2 trade ticks, no message.
    let mut markets = 0;
    let mut trades = 0;
    for _ in 0..4 {
        match &*next_frame(&mut all_rx).await {
            BroadcastFrame::Market(_) => markets += 1,
            BroadcastFrame::Trade(_) => trades += 1,
            other => panic!("AllMarkets is not the firehose, got {other:?}"),
        }
    }
    assert_eq!((markets, trades), (2, 2));
    assert_silent(&mut all_rx).await;

    drop(tx);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn failed_write_publishes_nothing_and_writer_survives() {
    let db = TestDb::new().await;
    let rooms = Rooms::new();
    let mut market_rx = rooms.subscribe(RoomKey::Market(pk58(1)));
    let mut all_rx = rooms.subscribe(RoomKey::AllMarkets);

    let (tx, rx) = mpsc::channel::<BlockBatch>(8);
    let writer = tokio::spawn(run_writer(db.pool.clone(), rx, rooms.clone()));

    // A trade for a market that does not exist: FK violation → the block's
    // transaction rolls back. No row, so no frame, and no checkpoint.
    tx.send(batch(
        100,
        vec![de(
            ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000),
            100,
            0,
        )],
    ))
    .await
    .unwrap();
    assert_silent(&mut market_rx).await;
    assert_silent(&mut all_rx).await;
    assert_eq!(
        checkpoint(&db).await,
        None,
        "failed block must not checkpoint"
    );

    // The writer is still alive: the next good block commits and publishes.
    tx.send(batch(101, vec![de(ev_market_created(1, 2), 101, 0)]))
        .await
        .unwrap();
    let f = next_frame(&mut market_rx).await;
    let BroadcastFrame::Market(m) = &*f else {
        panic!("got {f:?}")
    };
    assert_eq!(m.mint, pk58(1));
    assert_eq!(checkpoint(&db).await, Some(101));

    drop(tx);
    writer.await.unwrap().unwrap();
}
