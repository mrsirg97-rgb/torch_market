// Integration tests for the writer pipeline.
//
// Exercises the three-phase ordering (markets → pools → everything else),
// FK constraints across torch/deep_pool tables, idempotent replay, memo
// gating + attribution, V21 position delta application (CloseShort /
// LiquidateShort reconcile + position_events log), and checkpoint behavior.
//
// Each test gets a fresh DB. The writer is invoked via
// `write_events_no_checkpoint` for tests that don't care about
// `indexer_state` advancement, and via the full live path otherwise.

mod common;

use common::{fixtures::*, TestDb};

use sqlx::Row;

use torch_api::contracts::{
    AnyEvent, BlockBatch, BondingCurveTrade, CloseShortEvent, DecodedEvent, MarketStatus,
    PositionHealth, PositionSide, TorchEvent,
};
use torch_api::domain::{
    market, message, migration, pool, position, trade, MessageFilter, PositionEventFilter,
    TradeFilter,
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
    assert_eq!(m.status, MarketStatus::Bonding);

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
        torch_api::domain::PoolFilter {
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

// ─── OpenShort net semantics through the writer (V21) ───────────────────

#[tokio::test]
async fn writer_records_short_with_net_tokens_borrowed() {
    let db = TestDb::new().await;
    let net = 999_300_000u64;
    let events = vec![
        de(ev_market_created(1, 2), 100, 0),
        de(ev_open_short(1, 3, net), 100, 1),
    ];
    write_events_no_checkpoint(&db.pool, 100, events)
        .await
        .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &pk58(1), &pk58(3), PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.debt_amount, net as i64, "short debt = net tokens borrowed");
    assert!(p.is_active);
    assert!(!p.owner_is_vault);
    assert_eq!(p.health, PositionHealth::Healthy);

    // The open also appended a position_events row.
    let events = position::event::list(
        &mut tx,
        PositionEventFilter {
            mint: Some(pk58(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].tokens_out, Some(net as i64));
}

// ─── via_vault open sets owner_is_vault ─────────────────────────────────

#[tokio::test]
async fn writer_open_short_via_vault_flags_owner_is_vault() {
    let db = TestDb::new().await;
    let net = 999_300_000u64;
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de_via_vault(ev_open_short(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &pk58(1), &pk58(3), PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(p.owner_is_vault, "via_vault ix ⇒ owner_is_vault = true");
}

// ─── Delta application: CloseShort reconciles the position ──────────────

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
            de(ev_open_short(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Close fully — debt_repaid covers the borrowed tokens, fully_closed=true.
    let close_event = CloseShortEvent {
        user: pk(3),
        mint: pk(1),
        position_index: 0,
        debt_repaid: net,
        sol_spent_on_buyback: 1_500_000_000,
        interest_paid: 0,
        principal_paid: net, // token-denominated: the short's debt asset
        surplus_sol_to_user: 500_000_000,
        fully_closed: true,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![DecodedEvent {
            tx_idx: 0,
            signature: "sig_close".to_string(),
            inner_ix_idx: 0,
            slot: 200,
            block_time: Some(fixed_ts()),
            event: AnyEvent::Torch(TorchEvent::CloseShort(close_event)),
            memo: None,
            via_vault: false,
        }],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &pk58(1), &pk58(3), PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(!p.is_active, "fully_closed=true → is_active=false");
    assert_eq!(p.debt_amount, 0, "debt fully cleared");
    // [I-2] collateral_amount mirrors the on-chain STATIC record-keeping field
    // — the inactive row is the audit trail of what was deposited.
    assert_eq!(p.collateral_amount, 2_000_000_000, "audit value preserved");
    assert_eq!(p.vault_balance, 0, "vault drained on full close");
    assert_eq!(p.accrued_interest_stored, 0);
    assert_eq!(p.last_update_slot, 200);
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
            de(ev_open_short(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Pay back half.
    let close_event = CloseShortEvent {
        user: pk(3),
        mint: pk(1),
        position_index: 0,
        debt_repaid: net / 2,
        sol_spent_on_buyback: 1_000_000_000,
        interest_paid: 0,
        principal_paid: net / 2, // token-denominated
        surplus_sol_to_user: 0,
        fully_closed: false,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![DecodedEvent {
            tx_idx: 0,
            signature: "sig_partial".to_string(),
            inner_ix_idx: 0,
            slot: 200,
            block_time: Some(fixed_ts()),
            event: AnyEvent::Torch(TorchEvent::CloseShort(close_event)),
            memo: None,
            via_vault: false,
        }],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &pk58(1), &pk58(3), PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(p.is_active, "partial close → still active");
    // [I-2] debt -= principal_paid (NOT the interest-inclusive total).
    assert_eq!(p.debt_amount, (net - net / 2) as i64);
    // collateral static; vault -= sol spent on the buyback.
    assert_eq!(p.collateral_amount, 2_000_000_000);
    assert_eq!(p.vault_balance, 2_000_000_000 - 1_000_000_000);
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
                tx_idx: 0,
                signature: "zzz".to_string(),
                inner_ix_idx: 0,
                slot: 100,
                block_time: Some(fixed_ts()),
                event: ev_buy_trade(1, 3, 100_000, 50_000),
                memo: None,
                via_vault: false,
            },
            DecodedEvent {
                tx_idx: 0,
                signature: "aaa".to_string(),
                inner_ix_idx: 0,
                slot: 100,
                block_time: Some(fixed_ts()),
                event: ev_buy_trade(1, 3, 200_000, 100_000),
                memo: None,
                via_vault: false,
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

// ─── Liquidation delta application + analytics in position_events ───────

#[tokio::test]
async fn writer_short_opened_then_liquidated_in_full() {
    let db = TestDb::new().await;
    let mint = pk58(1);
    let borrower = pk58(3);
    let net = 999_300_000u64;

    // Step 1: create market + open short.
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(ev_open_short(1, 3, net), 100, 1),
        ],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let opened = position::get(&mut tx, &mint, &borrower, PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.debt_amount, net as i64);
    assert!(opened.is_active);
    drop(tx);

    // Step 2: liquidator (acct 9) fully liquidates.
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(
            ev_liquidate_short(1, 9, 3, net, true),
            200,
            0,
        )],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let liq = position::get(&mut tx, &mint, &borrower, PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(!liq.is_active);
    assert_eq!(liq.debt_amount, 0);
    // [I-2] static audit value preserved; the drained live asset is the vault.
    assert_eq!(liq.collateral_amount, 2_000_000_000);
    assert_eq!(liq.vault_balance, 0);
    assert_eq!(liq.health, PositionHealth::Liquidatable);

    // The liquidation analytics survive in the append-only log.
    let liqs = position::event::list(
        &mut tx,
        PositionEventFilter {
            mint: Some(mint.clone()),
            kind: Some(torch_api::contracts::PositionEventKind::Liquidate),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(liqs.len(), 1);
    assert_eq!(liqs[0].liquidator, Some(pk58(9)));
    assert_eq!(liqs[0].twap_ltv, Some(9200));
    assert_eq!(liqs[0].bonus_bps, Some(500));
    assert_eq!(liqs[0].seized, Some(1_900_000_000));
}

// ─── Suppress unused fixture imports (silenced through use). ────────────
#[allow(dead_code)]
fn _silence_unused(_: BondingCurveTrade, _: BlockBatch) {}

// ─── [I-2] Long-side coverage (previously ZERO tests at any layer) ───────

#[tokio::test]
async fn writer_long_open_close_partial_then_full() {
    use torch_api::contracts::{CloseLongEvent, OpenLongEvent};
    let db = TestDb::new().await;
    let mint = pk58(1);
    let owner = pk58(4);

    let open = OpenLongEvent {
        user: pk(4),
        mint: pk(1),
        position_index: 0,
        collateral_tokens: 5_000_000_000,
        borrowed_sol_gross: 3_000_000_000,
        open_fee_sol: 15_000_000,
        atomic_buy_sol: 2_985_000_000,
        vault_tokens: 11_000_000_000, // collateral + bought
    };
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(AnyEvent::Torch(TorchEvent::OpenLong(open)), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Partial close: sells 40% of vault, repays principal 1 SOL + interest 0.01.
    let partial = CloseLongEvent {
        user: pk(4),
        mint: pk(1),
        position_index: 0,
        tokens_sold: 4_400_000_000,
        sol_out: 1_300_000_000,
        debt_repaid: 1_010_000_000,
        interest_paid: 10_000_000,
        principal_paid: 1_000_000_000,
        surplus_sol_to_user: 290_000_000,
        fully_closed: false,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(AnyEvent::Torch(TorchEvent::CloseLong(partial)), 200, 0)],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &mint, &owner, PositionSide::Long, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(p.is_active);
    assert_eq!(p.debt_amount, 2_000_000_000, "debt -= principal_paid only");
    assert_eq!(p.collateral_amount, 5_000_000_000, "static audit value");
    assert_eq!(p.vault_balance, 11_000_000_000 - 4_400_000_000);
    drop(tx);

    // Full close.
    let full = CloseLongEvent {
        user: pk(4),
        mint: pk(1),
        position_index: 0,
        tokens_sold: 6_600_000_000,
        sol_out: 2_300_000_000,
        debt_repaid: 2_000_000_000,
        interest_paid: 0,
        principal_paid: 2_000_000_000,
        surplus_sol_to_user: 300_000_000,
        fully_closed: true,
    };
    write_events_no_checkpoint(
        &db.pool,
        300,
        vec![de(AnyEvent::Torch(TorchEvent::CloseLong(full)), 300, 0)],
    )
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &mint, &owner, PositionSide::Long, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(!p.is_active);
    assert_eq!(p.debt_amount, 0);
    assert_eq!(p.vault_balance, 0);
    assert_eq!(p.collateral_amount, 5_000_000_000);
}

#[tokio::test]
async fn writer_long_liquidation_splits_interest_first() {
    use torch_api::contracts::{LiquidateLongEvent, OpenLongEvent};
    let db = TestDb::new().await;
    let mint = pk58(1);
    let owner = pk58(4);

    let open = OpenLongEvent {
        user: pk(4),
        mint: pk(1),
        position_index: 0,
        collateral_tokens: 5_000_000_000,
        borrowed_sol_gross: 3_000_000_000,
        open_fee_sol: 15_000_000,
        atomic_buy_sol: 2_985_000_000,
        vault_tokens: 11_000_000_000,
    };
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 100, 0),
            de(AnyEvent::Torch(TorchEvent::OpenLong(open)), 100, 1),
        ],
    )
    .await
    .unwrap();

    // Partial liquidation in the SAME slot as the open's slot+0 interest → no
    // accrual; cover splits to principal entirely.
    let liq = LiquidateLongEvent {
        liquidator: pk(9),
        borrower: pk(4),
        mint: pk(1),
        position_index: 0,
        debt_covered: 1_500_000_000,
        tokens_seized: 6_000_000_000,
        bad_debt: 0,
        bonus_bps: 800,
        twap_ltv: 7000,
        fully_liquidated: false,
    };
    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![de(AnyEvent::Torch(TorchEvent::LiquidateLong(liq)), 100, 2)],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &mint, &owner, PositionSide::Long, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(p.is_active);
    assert_eq!(p.debt_amount, 1_500_000_000, "zero accrual → all principal");
    assert_eq!(p.vault_balance, 11_000_000_000 - 6_000_000_000);
    assert_eq!(p.health, PositionHealth::Liquidatable);
}

// ─── [I-1] Chain order within a block (signature-sort regression) ───────

#[tokio::test]
async fn writer_applies_same_block_open_then_close_in_chain_order() {
    // Two txs in ONE block: tx 0 opens, tx 1 fully closes. The signatures are
    // chosen so a signature sort would apply the CLOSE first (pre-fix: the
    // reconcile found no prior row, skipped, and the position showed open
    // forever). Chain order (tx_idx) must win.
    use torch_api::contracts::CloseShortEvent;
    let db = TestDb::new().await;
    let net = 999_300_000u64;

    let mut open_de = de(ev_open_short(1, 3, net), 100, 0);
    open_de.tx_idx = 0;
    open_de.signature = "zzz_open_sorts_last".to_string();
    let close = CloseShortEvent {
        user: pk(3),
        mint: pk(1),
        position_index: 0,
        debt_repaid: net,
        sol_spent_on_buyback: 1_500_000_000,
        interest_paid: 0,
        principal_paid: net,
        surplus_sol_to_user: 500_000_000,
        fully_closed: true,
    };
    let mut close_de = de(AnyEvent::Torch(TorchEvent::CloseShort(close)), 100, 0);
    close_de.tx_idx = 1;
    close_de.signature = "aaa_close_sorts_first".to_string();

    write_events_no_checkpoint(
        &db.pool,
        100,
        vec![
            de(ev_market_created(1, 2), 99, 0),
            close_de,
            open_de,
        ],
    )
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    let p = position::get(&mut tx, &pk58(1), &pk58(3), PositionSide::Short, 0)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !p.is_active,
        "chain order: open (tx 0) then close (tx 1) → position resolved"
    );
    assert_eq!(p.debt_amount, 0);
}

// ─── [lifecycle] status transitions ride program events ─────────────────

#[tokio::test]
async fn writer_lifecycle_status_transitions() {
    use torch_api::contracts::{BondingCompleted, MarketStatus, TokenReclaimed, TokenRevived};
    let db = TestDb::new().await;
    let mint = pk58(1);

    write_events_no_checkpoint(&db.pool, 100, vec![de(ev_market_created(1, 2), 100, 0)])
        .await
        .unwrap();

    // BONDING → COMPLETE
    let completed = BondingCompleted {
        mint: pk(1),
        real_sol_reserves: 100_000_000_000,
        bonding_complete_slot: 150,
    };
    write_events_no_checkpoint(
        &db.pool,
        150,
        vec![de(AnyEvent::Torch(TorchEvent::BondingCompleted(completed)), 150, 0)],
    )
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &mint).await.unwrap().unwrap();
    assert_eq!(m.status, MarketStatus::Complete);
    assert_eq!(m.bonding_complete_slot, Some(150));
    drop(tx);

    // (separate market) BONDING → RECLAIMED → revival → BONDING
    write_events_no_checkpoint(&db.pool, 100, vec![de(ev_market_created(5, 2), 100, 1)])
        .await
        .unwrap();
    let reclaimed = TokenReclaimed {
        mint: pk(5),
        sol_to_protocol_treasury: 1_000_000_000,
    };
    write_events_no_checkpoint(
        &db.pool,
        200,
        vec![de(AnyEvent::Torch(TorchEvent::TokenReclaimed(reclaimed)), 200, 0)],
    )
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &pk58(5)).await.unwrap().unwrap();
    assert_eq!(m.status, MarketStatus::Reclaimed);
    drop(tx);

    let revived = TokenRevived {
        mint: pk(5),
        total_contributed: 40_000_000_000,
        revival_slot: 300,
    };
    write_events_no_checkpoint(
        &db.pool,
        300,
        vec![de(AnyEvent::Torch(TorchEvent::TokenRevived(revived)), 300, 0)],
    )
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    let m = market::get_by_mint(&mut tx, &pk58(5)).await.unwrap().unwrap();
    assert_eq!(m.status, MarketStatus::Bonding, "revival resumes bonding");
}
