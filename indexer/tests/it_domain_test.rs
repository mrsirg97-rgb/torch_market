// Integration tests for the domain CRUD layer.
//
// Each test gets a fresh DB. Hits real SQL — covers query-builder correctness,
// FK constraints, ON CONFLICT semantics, and enum mapping that the
// pure-function tests can't reach.

mod common;

use common::{fixtures::*, TestDb};

use torch_indexer::contracts::{MarketStatus, MarketTier};
use torch_indexer::domain::{
    loan, market, message, migration, pool, short, swap, trade, MarketFilter, PoolFilter,
    ShortFilter, TradeFilter,
};

// ─── markets ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn market_set_then_get_by_mint() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();

    let mint = pk58(1);
    let creator = pk58(2);
    let row = new_market_row(&mint, &creator);
    let inserted = market::set(&mut tx, &row).await.unwrap();
    assert!(inserted.is_some());
    let inserted = inserted.unwrap();
    assert_eq!(inserted.mint, mint);
    assert_eq!(inserted.status, MarketStatus::Rs);
    assert_eq!(inserted.tier, MarketTier::Flame);

    let fetched = market::get_by_mint(&mut tx, &mint).await.unwrap().unwrap();
    assert_eq!(fetched.mint, mint);
    assert_eq!(fetched.virtual_sol, 30_000_000_000);

    tx.commit().await.unwrap();
}

#[tokio::test]
async fn market_set_idempotent_on_conflict() {
    // Re-inserting the same mint returns None (ON CONFLICT DO NOTHING).
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();

    let mint = pk58(1);
    let creator = pk58(2);
    let row = new_market_row(&mint, &creator);
    let first = market::set(&mut tx, &row).await.unwrap();
    assert!(first.is_some());

    let second = market::set(&mut tx, &row).await.unwrap();
    assert!(second.is_none(), "second insert must be a no-op");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn market_apply_trade_updates_reserves() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();

    market::apply_trade(
        &mut tx,
        &mint,
        31_000_000_000, // virtual_sol
        1_072_685_000_000_000, // virtual_token
        1_000_000_000, // real_sol
        800_000_000_000, // real_token
        200, // last_activity_slot
        fixed_ts(),
    )
    .await
    .unwrap();

    let fetched = market::get_by_mint(&mut tx, &mint).await.unwrap().unwrap();
    assert_eq!(fetched.virtual_sol, 31_000_000_000);
    assert_eq!(fetched.real_sol, 1_000_000_000);
    assert_eq!(fetched.last_activity_slot, 200);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn market_apply_migration_sets_status_and_fk() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    let pool_pk = pk58(50);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();
    // The pool row must exist first — markets.deep_pool_pubkey FKs to pools.pubkey.
    pool::set(&mut tx, &[new_pool_row(&pool_pk, &mint, &pk58(2))])
        .await
        .unwrap();

    market::apply_migration(&mut tx, &mint, &pool_pk, 500, fixed_ts())
        .await
        .unwrap();

    let fetched = market::get_by_mint(&mut tx, &mint).await.unwrap().unwrap();
    assert_eq!(fetched.status, MarketStatus::Migrated);
    assert_eq!(fetched.migrated_slot, Some(500));
    assert_eq!(fetched.deep_pool_pubkey, Some(pool_pk));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn market_list_filters_by_status() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();

    let mint_a = pk58(1);
    let mint_b = pk58(2);
    market::set(&mut tx, &new_market_row(&mint_a, &pk58(10)))
        .await
        .unwrap();
    market::set(&mut tx, &new_market_row(&mint_b, &pk58(11)))
        .await
        .unwrap();
    // Migrate B
    pool::set(&mut tx, &[new_pool_row(&pk58(50), &mint_b, &pk58(11))])
        .await
        .unwrap();
    market::apply_migration(&mut tx, &mint_b, &pk58(50), 200, fixed_ts())
        .await
        .unwrap();

    let rs_only = market::list(
        &mut tx,
        MarketFilter {
            status: Some(MarketStatus::Rs),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(rs_only.len(), 1);
    assert_eq!(rs_only[0].mint, mint_a);

    let migrated = market::list(
        &mut tx,
        MarketFilter {
            status: Some(MarketStatus::Migrated),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(migrated.len(), 1);
    assert_eq!(migrated[0].mint, mint_b);
    tx.commit().await.unwrap();
}

// ─── pools (deep_pool) ──────────────────────────────────────────────────

#[tokio::test]
async fn pool_set_returns_inserted_rows() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let pool_pk = pk58(10);
    let mint = pk58(11);
    let creator = pk58(12);

    let inserted = pool::set(&mut tx, &[new_pool_row(&pool_pk, &mint, &creator)])
        .await
        .unwrap();
    assert_eq!(inserted.len(), 1);
    assert_eq!(inserted[0].pubkey, pool_pk);
    assert!(inserted[0].pool_id > 0);

    // Replay → no new rows (ON CONFLICT DO NOTHING).
    let replay = pool::set(&mut tx, &[new_pool_row(&pool_pk, &mint, &creator)])
        .await
        .unwrap();
    assert!(replay.is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn pool_list_by_token_mint() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(11);
    pool::set(&mut tx, &[new_pool_row(&pk58(10), &mint, &pk58(12))])
        .await
        .unwrap();

    let rows = pool::list(
        &mut tx,
        PoolFilter {
            token_mints: Some(vec![mint.clone()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].token_mint, mint);
    tx.commit().await.unwrap();
}

// ─── trades ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn trade_set_requires_market_fk() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);

    // No markets row → trade insert FAILS (FK violation).
    let row = new_trade_row(&mint, &pk58(2), 100, 0);
    let result = trade::set(&mut tx, &[row]).await;
    assert!(result.is_err(), "trade without market FK should fail");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn trade_set_idempotent_on_replay() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();

    let row = new_trade_row(&mint, &pk58(3), 100, 0);
    let first = trade::set(&mut tx, &[row.clone()]).await.unwrap();
    assert_eq!(first.len(), 1);
    let second = trade::set(&mut tx, &[row]).await.unwrap();
    assert!(second.is_empty(), "replay of same (sig, inner) is no-op");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn trade_list_orders_newest_first() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();

    let trader = pk58(3);
    trade::set(
        &mut tx,
        &[
            new_trade_row(&mint, &trader, 100, 0),
            new_trade_row(&mint, &trader, 200, 0),
            new_trade_row(&mint, &trader, 150, 0),
        ],
    )
    .await
    .unwrap();

    let listed = trade::list(
        &mut tx,
        TradeFilter {
            mint: Some(mint.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(listed.len(), 3);
    // Ordered by slot DESC.
    assert_eq!(listed[0].slot, 200);
    assert_eq!(listed[1].slot, 150);
    assert_eq!(listed[2].slot, 100);
    tx.commit().await.unwrap();
}

// ─── loans (upsert / composite PK) ──────────────────────────────────────

#[tokio::test]
async fn loan_upsert_overwrites_existing() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();
    let borrower = pk58(3);

    let mut row = new_loan_row(&mint, &borrower, 100);
    let first = loan::upsert(&mut tx, &row).await.unwrap();
    assert_eq!(first.borrowed_amount, 2_000_000_000);
    assert!(first.is_active);

    // Repaid in full → second upsert with smaller amount + is_active=false.
    row.borrowed_amount = 0;
    row.collateral_amount = 0;
    row.is_active = false;
    row.last_update_slot = 200;
    let second = loan::upsert(&mut tx, &row).await.unwrap();
    assert_eq!(second.borrowed_amount, 0);
    assert!(!second.is_active);
    assert_eq!(second.last_update_slot, 200);
    assert_eq!(second.created_at, first.created_at, "PK row, created_at preserved");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn loan_get_by_composite_pk() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();
    let borrower_a = pk58(3);
    let borrower_b = pk58(4);
    loan::upsert(&mut tx, &new_loan_row(&mint, &borrower_a, 100))
        .await
        .unwrap();
    loan::upsert(&mut tx, &new_loan_row(&mint, &borrower_b, 100))
        .await
        .unwrap();

    let a = loan::get(&mut tx, &mint, &borrower_a).await.unwrap();
    let b = loan::get(&mut tx, &mint, &borrower_b).await.unwrap();
    assert!(a.is_some() && b.is_some());
    assert_ne!(a.unwrap().borrower, b.unwrap().borrower);

    let missing = loan::get(&mut tx, &mint, &pk58(99)).await.unwrap();
    assert!(missing.is_none());
    tx.commit().await.unwrap();
}

// ─── shorts ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn short_upsert_records_net_tokens_borrowed() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();
    let shorter = pk58(3);

    let row = new_short_row(&mint, &shorter, 100);
    let upserted = short::upsert(&mut tx, &row).await.unwrap();
    // The fixture uses 999_300_000 (gross 1B - 7bps fee = net).
    assert_eq!(upserted.tokens_borrowed, 999_300_000);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn short_list_active_only() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();

    let mut active = new_short_row(&mint, &pk58(10), 100);
    let mut closed = new_short_row(&mint, &pk58(11), 100);
    closed.is_active = false;
    short::upsert(&mut tx, &active).await.unwrap();
    short::upsert(&mut tx, &closed).await.unwrap();

    let only_active = short::list(
        &mut tx,
        ShortFilter {
            mint: Some(mint.clone()),
            is_active: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(only_active.len(), 1);
    assert_eq!(only_active[0].shorter, pk58(10));

    let all = short::list(
        &mut tx,
        ShortFilter {
            mint: Some(mint.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(all.len(), 2);

    // Quiet compiler about unused mut.
    active.is_active = true;
    let _ = active;
    tx.commit().await.unwrap();
}

// ─── messages ───────────────────────────────────────────────────────────

#[tokio::test]
async fn message_set_with_action_kind() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();

    let row = torch_indexer::contracts::NewMessageRow {
        mint: mint.clone(),
        sender: pk58(3),
        memo_text: "gm".to_string(),
        action_kind: Some("buy".to_string()),
        slot: 100,
        signature: "sig_msg_1".to_string(),
        inner_ix_idx: 0,
        created_at: fixed_ts(),
    };
    let inserted = message::set(&mut tx, &[row]).await.unwrap();
    assert_eq!(inserted.len(), 1);
    assert_eq!(inserted[0].memo_text, "gm");
    assert_eq!(inserted[0].action_kind, Some("buy".to_string()));
    tx.commit().await.unwrap();
}

// ─── migrations ─────────────────────────────────────────────────────────

#[tokio::test]
async fn migration_set_one_per_mint() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    let mint = pk58(1);
    let pool_pk = pk58(10);
    market::set(&mut tx, &new_market_row(&mint, &pk58(2)))
        .await
        .unwrap();
    pool::set(&mut tx, &[new_pool_row(&pool_pk, &mint, &pk58(2))])
        .await
        .unwrap();

    let row = torch_indexer::contracts::NewMigrationRow {
        mint: mint.clone(),
        deep_pool_pubkey: pool_pk.clone(),
        sol_seeded: 30_000_000_000,
        tokens_seeded: 1_073_000_000_000_000,
        lp_burned: 28_274_333,
        slot: 500,
        signature: "sig_mig".to_string(),
        created_at: fixed_ts(),
    };
    let first = migration::set(&mut tx, &row).await.unwrap();
    assert!(first.is_some());

    // ON CONFLICT (mint) DO NOTHING — second insert is a no-op.
    let second = migration::set(&mut tx, &row).await.unwrap();
    assert!(second.is_none());
    tx.commit().await.unwrap();
}

// ─── deep_pool: swap ────────────────────────────────────────────────────

#[tokio::test]
async fn swap_set_requires_pool_fk() {
    let db = TestDb::new().await;
    let mut tx = db.pool.begin().await.unwrap();
    // No pools row → swap insert FAILS (FK violation).
    let row = torch_indexer::contracts::NewSwapRow {
        pool_id: 9999,
        user_pk: pk58(1),
        sol_source: pk58(1),
        is_buy: true,
        amount_in_gross: 100,
        amount_in_net: 100,
        amount_out_gross: 50,
        amount_out_net: 50,
        fee: 0,
        sol_reserve_after: 100,
        token_reserve_after: 50,
        slot: 100,
        signature: "sig_swap_orphan".to_string(),
        inner_ix_idx: 0,
        created_at: fixed_ts(),
    };
    let result = swap::set(&mut tx, &[row]).await;
    assert!(result.is_err());
    tx.rollback().await.unwrap();
}
