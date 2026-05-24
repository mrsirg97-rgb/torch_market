// The writer task.
//
// Receives BlockBatches from the gRPC subscriber, writes each atomically
// (events + state mutations + checkpoint) in a single Postgres transaction,
// then broadcasts inserted rows to WS subscribers post-COMMIT.
//
// One BlockBatch = one transaction.
//
// Three-phase event processing per block, to satisfy FK constraints:
//   Phase 1: MarketCreated — `markets` rows must exist before trades/loans/
//            shorts/migrations referencing the mint.
//   Phase 2: PoolCreated — `pools` rows must exist before reserves/swaps/
//            liquidity referencing the pool, and before MigratedToDex sets
//            `markets.deep_pool_pubkey` (FK).
//   Phase 3: everything else, in (signature, inner_ix_idx) order.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::contracts::{
    AnyEvent, BlockBatch, BroadcastFrame, Broadcaster, DecodedEvent, DeepPoolEvent, LiquidityRow,
    LoanRow, MarketRow, MarketStatus, MessageRow, MigrationRow, NewLoanRow, NewMarketRow,
    NewMessageRow, NewShortRow, PoolRow, PositionHealth, ReservesRow, ShortRow, SwapRow, TorchEvent,
    TradeRow,
};
use crate::domain::{liquidity, loan, market, message, migration, pool, reserves, short, swap};
use crate::stream::translate;

pub async fn run_writer(
    db: PgPool,
    broadcaster: Broadcaster,
    mut rx: mpsc::Receiver<BlockBatch>,
) -> anyhow::Result<()> {
    info!("writer task started");

    let mut pool_cache = load_pool_cache(&db).await?;
    let mut lp_supply_cache = load_lp_supply_cache(&db).await?;
    info!(
        pools = pool_cache.len(),
        lp_supply_entries = lp_supply_cache.len(),
        "caches loaded"
    );

    while let Some(batch) = rx.recv().await {
        let slot = batch.slot;
        match write_block(&db, &mut pool_cache, &mut lp_supply_cache, &batch).await {
            Ok(written) => {
                // Live-only gauges. `events_total` + `blocks_written_total`
                // are incremented inside `write_block_inner` so backfill and
                // live ingest share the same accounting.
                crate::metrics::METRICS
                    .last_processed_slot
                    .set(slot as i64);
                crate::metrics::METRICS
                    .broadcast_subscribers
                    .set(broadcaster.subscriber_count() as i64);
                let n = written.total_count();
                if n > 0 {
                    info!(slot, inserted = n, "wrote block");
                }
                written.broadcast(&broadcaster);
            }
            Err(e) => {
                crate::metrics::METRICS.block_write_errors_total.inc();
                error!(slot, error = %e, "block write failed; checkpoint not advanced");
            }
        }
    }
    info!("writer task exiting (channel closed)");
    Ok(())
}

#[derive(Default)]
struct WrittenBlock {
    // deep_pool
    pools: Vec<PoolRow>,
    reserves: Vec<ReservesRow>,
    swaps: Vec<SwapRow>,
    liquidity: Vec<LiquidityRow>,
    // torch
    markets: Vec<MarketRow>,
    trades: Vec<TradeRow>,
    messages: Vec<MessageRow>,
    loans: Vec<LoanRow>,
    shorts: Vec<ShortRow>,
    migrations: Vec<MigrationRow>,
}

impl WrittenBlock {
    fn total_count(&self) -> usize {
        self.pools.len()
            + self.reserves.len()
            + self.swaps.len()
            + self.liquidity.len()
            + self.markets.len()
            + self.trades.len()
            + self.messages.len()
            + self.loans.len()
            + self.shorts.len()
            + self.migrations.len()
    }

    fn broadcast(self, bc: &Broadcaster) {
        for r in self.pools {
            bc.publish(BroadcastFrame::Pool(Arc::new(r)));
        }
        for r in self.reserves {
            bc.publish(BroadcastFrame::Reserves(Arc::new(r)));
        }
        for r in self.swaps {
            bc.publish(BroadcastFrame::Swap(Arc::new(r)));
        }
        for r in self.liquidity {
            bc.publish(BroadcastFrame::Liquidity(Arc::new(r)));
        }
        for r in self.markets {
            bc.publish(BroadcastFrame::Market(Arc::new(r)));
        }
        for r in self.trades {
            bc.publish(BroadcastFrame::Trade(Arc::new(r)));
        }
        for r in self.messages {
            bc.publish(BroadcastFrame::Message(Arc::new(r)));
        }
        for r in self.loans {
            bc.publish(BroadcastFrame::Loan(Arc::new(r)));
        }
        for r in self.shorts {
            bc.publish(BroadcastFrame::Short(Arc::new(r)));
        }
        for r in self.migrations {
            bc.publish(BroadcastFrame::Migration(Arc::new(r)));
        }
    }
}

// Public helper for the backfill path: same event-application logic as
// write_block, but takes ownership of events directly (no BlockBatch
// indirection) and DOES NOT touch indexer_state. Returns the count of newly
// inserted rows. Each call is one Postgres transaction.
pub async fn write_events_no_checkpoint(
    db: &PgPool,
    slot: u64,
    events: Vec<DecodedEvent>,
) -> anyhow::Result<usize> {
    // Backfill rebuilds caches lazily per slot. Cheaper than carrying them
    // across the whole backfill (which may span tens of thousands of slots)
    // since each slot's writer only needs pool_id resolution for the events
    // it actually contains.
    let mut pool_cache = HashMap::new();
    let mut lp_supply_cache = HashMap::new();
    let batch = BlockBatch { slot, events };
    let written =
        write_block_inner(db, &mut pool_cache, &mut lp_supply_cache, &batch, false).await?;
    Ok(written.total_count())
}

async fn write_block(
    db: &PgPool,
    pool_cache: &mut HashMap<String, i32>,
    lp_supply_cache: &mut HashMap<i32, i64>,
    batch: &BlockBatch,
) -> anyhow::Result<WrittenBlock> {
    write_block_inner(db, pool_cache, lp_supply_cache, batch, true).await
}

async fn write_block_inner(
    db: &PgPool,
    pool_cache: &mut HashMap<String, i32>,
    lp_supply_cache: &mut HashMap<i32, i64>,
    batch: &BlockBatch,
    update_checkpoint: bool,
) -> anyhow::Result<WrittenBlock> {
    let mut tx = db.begin().await?;
    let mut out = WrittenBlock::default();

    // Per-event counts BEFORE writing — counts what the indexer SAW, not
    // what successfully landed. A divergence between this counter and
    // actual table row counts is the canary for silent writer drops.
    // Placed here (in write_block_inner) so backfill and live ingest share
    // the accounting.
    for de in &batch.events {
        let (program, kind) = crate::metrics::event_labels(&de.event);
        crate::metrics::METRICS
            .events_total
            .with_label_values(&[program, kind])
            .inc();
    }

    // Deterministic processing order.
    let mut events: Vec<&DecodedEvent> = batch.events.iter().collect();
    events.sort_by(|a, b| {
        a.signature
            .cmp(&b.signature)
            .then(a.inner_ix_idx.cmp(&b.inner_ix_idx))
    });

    // ─── Phase 1: MarketCreated ──────────────────────────────────────────
    for de in &events {
        if let AnyEvent::Torch(TorchEvent::MarketCreated(m)) = &de.event {
            let row = translate::new_market(m, de);
            if let Some(inserted) = market::set(&mut tx, &row).await? {
                out.markets.push(inserted);
            }
        }
    }

    // ─── Phase 2: PoolCreated ────────────────────────────────────────────
    let new_pools: Vec<_> = events
        .iter()
        .filter_map(|de| match &de.event {
            AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(p)) => {
                Some(translate::new_pool(p, de))
            }
            _ => None,
        })
        .collect();
    let inserted_pools = pool::set(&mut tx, &new_pools).await?;
    for r in &inserted_pools {
        pool_cache.insert(r.pubkey.clone(), r.pool_id);
    }
    out.pools.extend(inserted_pools);

    // Backfill pool_cache for events referencing pools we don't know about
    // yet (replay after restart with new pools, or events for pools created
    // in this same block).
    backfill_pool_cache(&mut tx, &events, pool_cache).await?;

    // ─── Phase 3: everything else ───────────────────────────────────────
    for de in &events {
        match &de.event {
            // Already handled in phase 1/2 — skip.
            AnyEvent::Torch(TorchEvent::MarketCreated(_)) => continue,
            AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(_)) => {
                // PoolCreated also writes an initial reserves snapshot.
                if let AnyEvent::DeepPool(DeepPoolEvent::PoolCreated(p)) = &de.event {
                    let pubkey = translate::b58(&p.pool);
                    if let Some(&pool_id) = pool_cache.get(&pubkey) {
                        lp_supply_cache.insert(pool_id, p.lp_supply_after as i64);
                        let r = translate::new_reserves(
                            pool_id,
                            p.sol_reserve_after as i64,
                            p.token_reserve_after as i64,
                            p.lp_supply_after as i64,
                            de,
                        );
                        let inserted = reserves::set(&mut tx, &[r]).await?;
                        out.reserves.extend(inserted);
                    }
                }
                continue;
            }
            AnyEvent::DeepPool(ev) => {
                write_deep_pool_event(
                    &mut tx,
                    de,
                    ev,
                    pool_cache,
                    lp_supply_cache,
                    &mut out,
                )
                .await?;
            }
            AnyEvent::Torch(ev) => {
                write_torch_event(&mut tx, de, ev, &mut out).await?;
            }
        }
    }

    if update_checkpoint {
        sqlx::query(
            "INSERT INTO indexer_state (id, last_processed_slot) VALUES (1, $1)
             ON CONFLICT (id) DO UPDATE SET last_processed_slot = EXCLUDED.last_processed_slot",
        )
        .bind(batch.slot as i64)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    // Successful commit. Live ingest also bumps `last_processed_slot` /
    // `broadcast_subscribers` in run_writer after this returns.
    crate::metrics::METRICS.blocks_written_total.inc();
    Ok(out)
}

async fn backfill_pool_cache(
    tx: &mut Transaction<'_, Postgres>,
    events: &[&DecodedEvent],
    pool_cache: &mut HashMap<String, i32>,
) -> anyhow::Result<()> {
    use crate::domain::pool::PoolFilter;
    let mut needed: Vec<String> = events
        .iter()
        .filter_map(|de| match &de.event {
            AnyEvent::DeepPool(ev) => Some(translate::pool_pubkey(ev)),
            _ => None,
        })
        .filter(|pk| !pool_cache.contains_key(pk))
        .collect();
    needed.sort();
    needed.dedup();
    if !needed.is_empty() {
        let rows = pool::list(
            tx,
            PoolFilter {
                pubkeys: Some(needed),
                ..Default::default()
            },
        )
        .await?;
        for r in rows {
            pool_cache.insert(r.pubkey.clone(), r.pool_id);
        }
    }
    Ok(())
}

async fn write_deep_pool_event(
    tx: &mut Transaction<'_, Postgres>,
    de: &DecodedEvent,
    ev: &DeepPoolEvent,
    pool_cache: &HashMap<String, i32>,
    lp_supply_cache: &mut HashMap<i32, i64>,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let pubkey = translate::pool_pubkey(ev);
    let Some(&pool_id) = pool_cache.get(&pubkey) else {
        warn!(slot = de.slot, sig = %de.signature, %pubkey, "unknown pool; skipping deep_pool event");
        return Ok(());
    };
    let now = translate::ts(de);
    match ev {
        DeepPoolEvent::PoolCreated(_) => unreachable!("handled in phase 2"),
        DeepPoolEvent::SwapExecuted(s) => {
            let lp_supply = lp_supply_cache.get(&pool_id).copied().unwrap_or(0);
            let inserted_swaps = swap::set(tx, &[translate::new_swap(s, pool_id, de)]).await?;
            out.swaps.extend(inserted_swaps);
            let inserted_reserves = reserves::set(
                tx,
                &[translate::new_reserves(
                    pool_id,
                    s.sol_reserve_after as i64,
                    s.token_reserve_after as i64,
                    lp_supply,
                    de,
                )],
            )
            .await?;
            out.reserves.extend(inserted_reserves);

            // Memo attribution for post-migration trades. The grpc decoder
            // attaches memos to either BondingCurveTrade or SwapExecuted,
            // so DEX trades carry messages too. Look up token_mint from the
            // pool — direct SQL since pool_cache is pubkey-keyed and most
            // swaps don't carry memos (cheap when it skips).
            if let Some(memo_text) = &de.memo {
                let mint: Option<String> =
                    sqlx::query_scalar("SELECT token_mint FROM pools WHERE pool_id = $1")
                        .bind(pool_id)
                        .fetch_optional(&mut **tx)
                        .await?;
                if let Some(mint) = mint {
                    let row = NewMessageRow {
                        mint,
                        sender: translate::b58(&s.user),
                        memo_text: memo_text.clone(),
                        action_kind: Some(if s.buy { "buy" } else { "sell" }.to_string()),
                        slot: de.slot,
                        signature: de.signature.clone(),
                        inner_ix_idx: de.inner_ix_idx,
                        created_at: now,
                    };
                    let inserted = message::set(tx, &[row]).await?;
                    out.messages.extend(inserted);
                }
            }
        }
        DeepPoolEvent::LiquidityAdded(la) => {
            lp_supply_cache.insert(pool_id, la.lp_supply_after as i64);
            let inserted = liquidity::set(tx, &[translate::new_liquidity_add(la, pool_id, de)]).await?;
            out.liquidity.extend(inserted);
            let inserted_reserves = reserves::set(
                tx,
                &[translate::new_reserves(
                    pool_id,
                    la.sol_reserve_after as i64,
                    la.token_reserve_after as i64,
                    la.lp_supply_after as i64,
                    de,
                )],
            )
            .await?;
            out.reserves.extend(inserted_reserves);
        }
        DeepPoolEvent::LiquidityRemoved(lr) => {
            lp_supply_cache.insert(pool_id, lr.lp_supply_after as i64);
            let inserted = liquidity::set(tx, &[translate::new_liquidity_remove(lr, pool_id, de)]).await?;
            out.liquidity.extend(inserted);
            let inserted_reserves = reserves::set(
                tx,
                &[translate::new_reserves(
                    pool_id,
                    lr.sol_reserve_after as i64,
                    lr.token_reserve_after as i64,
                    lr.lp_supply_after as i64,
                    de,
                )],
            )
            .await?;
            out.reserves.extend(inserted_reserves);
        }
    }
    Ok(())
}

async fn write_torch_event(
    tx: &mut Transaction<'_, Postgres>,
    de: &DecodedEvent,
    ev: &TorchEvent,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let now = translate::ts(de);
    match ev {
        TorchEvent::MarketCreated(_) => unreachable!("handled in phase 1"),

        TorchEvent::BondingCurveTrade(t) => {
            let mint = translate::b58(&t.mint);
            // 1. Append trade row.
            let inserted = swap_trade(tx, t, de).await?;
            out.trades.extend(inserted);

            // 2. Update markets (reserves + last_activity_slot).
            market::apply_trade(
                tx,
                &mint,
                t.virtual_sol_after as i64,
                t.virtual_token_after as i64,
                t.real_sol_after as i64,
                t.real_token_after as i64,
                de.slot,
                now,
            )
            .await?;

            // 3. Memo (if attached by grpc.rs gating).
            if let Some(memo_text) = &de.memo {
                let row = NewMessageRow {
                    mint: mint.clone(),
                    sender: translate::b58(&t.trader),
                    memo_text: memo_text.clone(),
                    action_kind: Some(if t.is_buy { "buy" } else { "sell" }.to_string()),
                    slot: de.slot,
                    signature: de.signature.clone(),
                    inner_ix_idx: de.inner_ix_idx,
                    created_at: now,
                };
                let inserted = message::set(tx, &[row]).await?;
                out.messages.extend(inserted);
            }
        }

        TorchEvent::MigratedToDex(m) => {
            let mint = translate::b58(&m.mint);
            let deep_pool_pubkey = translate::b58(&m.deep_pool);
            // Markets FK + migrations event row in the same txn — both
            // visible together or neither.
            market::apply_migration(tx, &mint, &deep_pool_pubkey, de.slot, now).await?;
            let row = translate::new_migration(m, de);
            if let Some(inserted) = migration::set(tx, &row).await? {
                out.migrations.push(inserted);
            }
        }

        TorchEvent::VaultSwapExecuted(_) => {
            // VaultSwap is a CPI orchestration event. The underlying trade
            // is already captured as either a BondingCurveTrade (pre-mig)
            // or a deep_pool SwapExecuted (post-mig). No table mutation.
        }

        TorchEvent::LoanCreated(e) => {
            let row = translate::new_loan_from_created(e, de);
            let inserted = loan::upsert(tx, &row).await?;
            out.loans.push(inserted);
        }
        TorchEvent::LoanRepaid(e) => {
            apply_loan_repaid(tx, e, de, &mut out.loans).await?;
        }
        TorchEvent::LoanLiquidated(e) => {
            apply_loan_liquidated(tx, e, de, &mut out.loans).await?;
        }

        TorchEvent::ShortOpened(e) => {
            let row = translate::new_short_from_opened(e, de);
            let inserted = short::upsert(tx, &row).await?;
            out.shorts.push(inserted);
        }
        TorchEvent::ShortClosed(e) => {
            apply_short_closed(tx, e, de, &mut out.shorts).await?;
        }
        TorchEvent::ShortLiquidated(e) => {
            apply_short_liquidated(tx, e, de, &mut out.shorts).await?;
        }

        TorchEvent::RevivalContribution(_) | TorchEvent::TokenRevived(_) => {
            // Revival events affect bonding curve state but don't currently
            // have a dedicated table. Reserves snapshot updates flow through
            // the next BondingCurveTrade after revival. Revisit if a
            // `revivals` table is added.
        }
    }
    Ok(())
}

// Local helper: insert a trade row via the domain layer.
async fn swap_trade(
    tx: &mut Transaction<'_, Postgres>,
    t: &crate::contracts::BondingCurveTrade,
    de: &DecodedEvent,
) -> anyhow::Result<Vec<TradeRow>> {
    use crate::domain::trade;
    let row = translate::new_trade(t, de);
    Ok(trade::set(tx, &[row]).await?)
}

// ───────────── Loan / Short delta application ───────────────────────────
//
// LoanRepaid / LoanLiquidated / ShortClosed / ShortLiquidated carry deltas,
// not absolute amounts. Read the prior row, apply the diff, upsert. If the
// prior row is missing (event arrived before its OPEN — backfill ordering
// edge case), log a warning and skip.

async fn apply_loan_repaid(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::LoanRepaid,
    de: &DecodedEvent,
    out: &mut Vec<LoanRow>,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let borrower = translate::b58(&e.user);
    let Some(prior) = loan::get(tx, &mint, &borrower).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %borrower, "LoanRepaid for unknown loan; skipping");
        return Ok(());
    };
    let new_borrowed = (prior.borrowed_amount - e.sol_repaid as i64).max(0);
    let new_collateral = (prior.collateral_amount - e.collateral_returned as i64).max(0);
    let now = translate::ts(de);
    let row = NewLoanRow {
        mint,
        borrower,
        collateral_amount: new_collateral,
        borrowed_amount: new_borrowed,
        accrued_interest_stored: 0, // reset on every repay (interest was paid)
        last_update_slot: de.slot,
        health: PositionHealth::Healthy,
        is_active: !e.fully_repaid,
        created_at: prior.created_at,
        updated_at: now,
    };
    out.push(loan::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_loan_liquidated(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::LoanLiquidated,
    de: &DecodedEvent,
    out: &mut Vec<LoanRow>,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let borrower = translate::b58(&e.borrower);
    let Some(prior) = loan::get(tx, &mint, &borrower).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %borrower, "LoanLiquidated for unknown loan; skipping");
        return Ok(());
    };
    let now = translate::ts(de);
    let row = NewLoanRow {
        mint,
        borrower,
        collateral_amount: (prior.collateral_amount - e.collateral_seized as i64).max(0),
        borrowed_amount: (prior.borrowed_amount - e.debt_covered as i64).max(0),
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        // Position fully liquidated when debt fully covered and no bad debt
        // remains, OR when borrowed_amount reaches 0. Mark inactive then.
        health: PositionHealth::Liquidatable,
        is_active: prior.borrowed_amount > e.debt_covered as i64,
        created_at: prior.created_at,
        updated_at: now,
    };
    out.push(loan::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_short_closed(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::ShortClosed,
    de: &DecodedEvent,
    out: &mut Vec<ShortRow>,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let shorter = translate::b58(&e.user);
    let Some(prior) = short::get(tx, &mint, &shorter).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %shorter, "ShortClosed for unknown short; skipping");
        return Ok(());
    };
    let now = translate::ts(de);
    // tokens_returned reduces tokens_borrowed (which is net). sol_returned
    // reduces sol_collateral.
    let row = NewShortRow {
        mint,
        shorter,
        sol_collateral: (prior.sol_collateral - e.sol_returned as i64).max(0),
        tokens_borrowed: (prior.tokens_borrowed - e.tokens_returned as i64).max(0),
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        health: PositionHealth::Healthy,
        is_active: !e.fully_closed,
        created_at: prior.created_at,
        updated_at: now,
    };
    out.push(short::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_short_liquidated(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::ShortLiquidated,
    de: &DecodedEvent,
    out: &mut Vec<ShortRow>,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let shorter = translate::b58(&e.borrower);
    let Some(prior) = short::get(tx, &mint, &shorter).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %shorter, "ShortLiquidated for unknown short; skipping");
        return Ok(());
    };
    let now = translate::ts(de);
    let row = NewShortRow {
        mint,
        shorter,
        sol_collateral: (prior.sol_collateral - e.sol_seized as i64).max(0),
        tokens_borrowed: (prior.tokens_borrowed - e.tokens_covered as i64).max(0),
        accrued_interest_stored: 0,
        last_update_slot: de.slot,
        health: PositionHealth::Liquidatable,
        is_active: prior.tokens_borrowed > e.tokens_covered as i64,
        created_at: prior.created_at,
        updated_at: now,
    };
    out.push(short::upsert(tx, &row).await?);
    Ok(())
}

// ───────────── Cache loaders ────────────────────────────────────────────

async fn load_pool_cache(db: &PgPool) -> sqlx::Result<HashMap<String, i32>> {
    let rows: Vec<(String, i32)> = sqlx::query_as("SELECT pubkey, pool_id FROM pools")
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

async fn load_lp_supply_cache(db: &PgPool) -> sqlx::Result<HashMap<i32, i64>> {
    let rows: Vec<(i32, i64)> = sqlx::query_as(
        "SELECT DISTINCT ON (pool_id) pool_id, lp_supply
         FROM reserves
         ORDER BY pool_id, last_slot DESC, reserve_id DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

// Quiet a couple of unused imports that may be needed once revivals/etc.
// gain dedicated tables.
#[allow(dead_code)]
fn _unused_imports_sink(_: DateTime<Utc>, _: NewMarketRow, _: MarketStatus) {}
