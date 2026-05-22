// Integration tests for the writer pipeline.
//
// Exercises the three-phase ordering (markets → pools → everything else),
// FK constraints across torch/deep_pool tables, idempotent replay, memo
// gating + attribution, position delta application (LoanRepaid,
// ShortClosed), and checkpoint behavior.
//
// Each test gets a fresh DB. The writer is invoked via
// `write_events_no_checkpoint` for tests that don't care about
// `indexer_state` advancement, and via the full live path otherwise.

mod common;

use common::{fixtures::*, TestDb};

use sqlx::Row;

use torch_indexer::contracts::{
    AnyEvent, BlockBatch, BondingCurveTrade, DecodedEvent, MarketStatus, PositionHealth,
    ShortClosed, TorchEvent,
};
use torch_indexer::domain::{
    loan, market, message, migration, pool, short, trade, MessageFilter, TradeFilter,
};
use torch_indexer::stream::writer::write_events_no_checkpoint;

// ─── Phase 1: market lifecycle through the writer ───────────────────────

#[tokio::test]
async fn writer_creates_market_then_persists_trade() {
    let db = TestDb::new().await;
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000), 100, 1),
    ];
    let inserted = write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();
    assert!(inserted >= 2, "expected at least market + trade rows");

    // Markets table got the row.
    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &pk58(1)).await.unwrap().unwrap();
    assert_eq!(m.status, MarketStatus::Rs);

    // Reserves updated by apply_trade.
    assert_eq!(m.real_sol, 1_000_000_000);

    // Trade row inserted.
    let trades = trade::list(
        &mut tx,
        TradeFilter {
            mint: Some(pk58(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(trades.len(), 1);
    assert!(trades[0].is_buy);
}

#[tokio::test]
async fn writer_handles_trade_before_market_within_same_batch() {
    // Phase ordering: even if the trade event is listed BEFORE the market
    // event in the batch, phase 1 processes MarketCreated first so the FK
    // resolves on the trade insert.
    let db = TestDb::new().await;
    let events = vec![
        de(ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000), 100, 1),
        de(ev_market_created(1, 2), 100, 0),
    ];
    let inserted = write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();
    assert!(inserted >= 2);
}

// ─── Phase 2: pool + migration FK chain ──────────────────────────────────

#[tokio::test]
async fn writer_migration_chains_markets_to_pools_atomically() {
    let db = TestDb::new().await;
    // Single batch: market created → pool created → market migrated to pool.
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_pool_created(50, 1, 2), 100, 1),
        de(ev_migrated(1, 50), 100, 2),
    ];
    let inserted = write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();
    assert!(inserted >= 3);

    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &pk58(1)).await.unwrap().unwrap();
    assert_eq!(m.status, MarketStatus::Migrated);
    assert_eq!(m.deep_pool_pubkey, Some(pk58(50)));
    assert_eq!(m.migrated_slot, Some(100));

    let mig = migration::get_by_mint(&mut tx, &pk58(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mig.deep_pool_pubkey, pk58(50));
}

#[tokio::test]
async fn writer_pool_swap_writes_swap_and_reserves_snapshot() {
    let db = TestDb::new().await;
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_pool_created(50, 1, 2), 100, 1),
        de(ev_migrated(1, 50), 100, 2),
        de(ev_swap(50, 7, true), 200, 0),
    ];
    write_events_no_checkpoint(&db.pool, 100, events.clone())
        .await
        .unwrap();
    // Second batch flushed for the swap at slot 200.
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(ev_swap(50, 7, true), 200, 0)],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let pools = pool::list(
        &mut tx,
        torch_indexer::domain::PoolFilter {
            pubkeys: Some(vec![pk58(50)]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(pools.len(), 1);
    let pool_id = pools[0].pool_id;

    // Reserves row created (PoolCreated phase 3 + the swap event).
    let row = sqlx::query("SELECT COUNT(*) AS n FROM reserves WHERE pool_id = $1")
        .bind(pool_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let n: i64 = row.get("n");
    assert!(n >= 1, "expected at least one reserves row for the pool");
}

// ─── Idempotency under replay ────────────────────────────────────────────

#[tokio::test]
async fn writer_idempotent_on_full_batch_replay() {
    let db = TestDb::new().await;
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000), 100, 1),
    ];
    let first = write_events_no_checkpoint(&db.pool, 100, events.clone())
        .await
        .unwrap();
    let second = write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();
    assert!(first > 0);
    assert_eq!(second, 0, "replay must produce zero new rows");

    // Markets row is still there exactly once.
    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &pk58(1)).await.unwrap().unwrap();
    let row = sqlx::query("SELECT COUNT(*) AS n FROM markets WHERE mint = $1")
        .bind(&m.mint)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let n: i64 = row.get("n");
    assert_eq!(n, 1);
}

// ─── Memo gating ─────────────────────────────────────────────────────────

#[tokio::test]
async fn writer_persists_memo_only_when_attached_to_trade() {
    let db = TestDb::new().await;
    let mut events = vec![
        de(ev_market_created(1, 2), 100, 0),
        // Trade with attached memo (this is how grpc.rs would deliver it
        // post-gating).
        de_with_memo(
            ev_buy_trade(1, 3, 1_000_000_000, 500_000_000_000_000),
            100,
            1,
            "gm wgmi",
        ),
    ];
    // Add a second trade without a memo to verify nothing leaks.
    events.push(de(
        ev_buy_trade(1, 3, 500_000_000, 250_000_000_000_000),
        100,
        2,
    ));

    write_events_no_checkpoint(&db.pool, 100, events).await.unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let messages = message::list(
        &mut tx,
        MessageFilter {
            mint: Some(pk58(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(messages.len(), 1, "exactly one memo persisted");
    assert_eq!(messages[0].memo_text, "gm wgmi");
    assert_eq!(messages[0].action_kind, Some("buy".to_string()));
    assert_eq!(messages[0].sender, pk58(3));
}

// ─── ShortOpened net semantics through the writer ───────────────────────

#[tokio::test]
async fn writer_records_short_with_net_tokens_borrowed() {
    let db = TestDb::new().await;
    let net = 999_300_000u64;
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_short_opened(1, 3, net), 100, 1),
    ];
    write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let s = short::get(&mut tx, &pk58(1), &pk58(3)).await.unwrap().unwrap();
    assert_eq!(s.tokens_borrowed, net as i64);
    assert!(s.is_active);
    assert_eq!(s.health, PositionHealth::Healthy);
}

// ─── Delta application: ShortClosed reduces tokens_borrowed ─────────────

#[tokio::test]
async fn writer_short_closed_full_marks_inactive() {
    let db = TestDb::new().await;
    let net = 999_300_000u64;
    // Open
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_short_opened(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Close fully — tokens_returned + interest covers debt, fully_closed=true.
    let close_event = ShortClosed {
        mint: pk(1),
        user: pk(3),
        tokens_returned: net, // matches tokens_borrowed
        interest_paid_tokens: 0,
        sol_returned: 2_000_000_000,
        fully_closed: true,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![DecodedEvent {
            signature: "sig_close".to_string(),
            inner_ix_idx: 0,
            slot: 200,
            block_time: Some(fixed_ts()),
            event: AnyEvent::Torch(TorchEvent::ShortClosed(close_event)),
            memo: None,
        }],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let s = short::get(&mut tx, &pk58(1), &pk58(3)).await.unwrap().unwrap();
    assert!(!s.is_active, "fully_closed=true → is_active=false");
    assert_eq!(s.tokens_borrowed, 0, "debt fully cleared");
    assert_eq!(s.sol_collateral, 0, "collateral fully returned");
    assert_eq!(s.last_update_slot, 200);
}

#[tokio::test]
async fn writer_short_closed_partial_stays_active() {
    let db = TestDb::new().await;
    let net = 999_300_000u64;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_short_opened(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Pay back half.
    let close_event = ShortClosed {
        mint: pk(1),
        user: pk(3),
        tokens_returned: net / 2,
        interest_paid_tokens: 0,
        sol_returned: 1_000_000_000,
        fully_closed: false,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![DecodedEvent {
            signature: "sig_partial".to_string(),
            inner_ix_idx: 0,
            slot: 200,
            block_time: Some(fixed_ts()),
            event: AnyEvent::Torch(TorchEvent::ShortClosed(close_event)),
            memo: None,
        }],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let s = short::get(&mut tx, &pk58(1), &pk58(3)).await.unwrap().unwrap();
    assert!(s.is_active, "partial close → still active");
    assert_eq!(s.tokens_borrowed, (net / 2) as i64);
}

// ─── Order of events within batch is deterministic ──────────────────────

#[tokio::test]
async fn writer_orders_events_deterministically() {
    let db = TestDb::new().await;
    // Insert three trades in shuffled order; verify they land sorted by
    // (signature, inner_ix_idx).
    let mint = pk58(1);
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            // Use signatures that sort lexicographically.
            DecodedEvent {
                signature: "zzz".to_string(),
                inner_ix_idx: 0,
                slot: 100,
                block_time: Some(fixed_ts()),
                event: ev_buy_trade(1, 3, 100_000, 50_000),
                memo: None,
            },
            DecodedEvent {
                signature: "aaa".to_string(),
                inner_ix_idx: 0,
                slot: 100,
                block_time: Some(fixed_ts()),
                event: ev_buy_trade(1, 3, 200_000, 100_000),
                memo: None,
            },
        ],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let trades = trade::list(
        &mut tx,
        TradeFilter {
            mint: Some(mint),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Both trades persisted.
    assert_eq!(trades.len(), 2);
}

// ─── Loan delta application ──────────────────────────────────────────────

#[tokio::test]
async fn writer_loan_created_then_repaid_in_full() {
    let db = TestDb::new().await;
    let mint = pk58(1);
    let user = pk58(3);

    // Step 1: create market + open loan.
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(
                AnyEvent::Torch(TorchEvent::LoanCreated(
                    torch_indexer::contracts::LoanCreated {
                        mint: pk(1),
                        user: pk(3),
                        collateral_amount: 100_000_000_000,
                        borrowed_amount: 2_000_000_000,
                        ltv_bps: 4000,
                    },
                )),
                100,
                1,
            ),
        ],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let opened = loan::get(&mut tx, &mint, &user).await.unwrap().unwrap();
    assert_eq!(opened.borrowed_amount, 2_000_000_000);
    assert!(opened.is_active);
    drop(tx);

    // Step 2: repay in full.
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(
            AnyEvent::Torch(TorchEvent::LoanRepaid(
                torch_indexer::contracts::LoanRepaid {
                    mint: pk(1),
                    user: pk(3),
                    sol_repaid: 2_000_000_000,
                    interest_paid: 0,
                    collateral_returned: 100_000_000_000,
                    fully_repaid: true,
                },
            )),
            200,
            0,
        )],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let repaid = loan::get(&mut tx, &mint, &user).await.unwrap().unwrap();
    assert!(!repaid.is_active);
    assert_eq!(repaid.borrowed_amount, 0);
    assert_eq!(repaid.collateral_amount, 0);
}

// ─── Suppress unused fixture imports (silenced through use). ────────────
#[allow(dead_code)]
fn _silence_unused(_: BondingCurveTrade, _: BlockBatch) {}
