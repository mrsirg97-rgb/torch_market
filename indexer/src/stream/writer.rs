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

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::contracts::{
    AnyEvent, BlockBatch, DecodedEvent, DeepPoolEvent, LiquidityRow,
    MarketRow, MarketStatus, MessageRow, MigrationRow, NewMarketRow, NewMessageRow, NewPositionRow,
    PoolRow, PositionEventRow, PositionHealth, PositionRow, PositionSide, ReservesRow, SwapRow,
    TorchEvent, TradeRow,
};
use crate::domain::{liquidity, market, message, migration, pool, position, reserves, swap};
use crate::stream::translate;

pub async fn run_writer(db: PgPool, mut rx: mpsc::Receiver<BlockBatch>) -> anyhow::Result<()> {
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
                let n = written.total_count();
                if n > 0 {
                    info!(slot, inserted = n, "wrote block");
                }
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
    positions: Vec<PositionRow>,
    position_events: Vec<PositionEventRow>,
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
            + self.positions.len()
            + self.position_events.len()
            + self.migrations.len()
    }

    // [prompt-003] Queue one thin pg_notify per inserted row INSIDE the write
    // transaction — Postgres delivers on COMMIT (never for rollbacks, in
    // commit order). Payload is always thin: {"t": table, "k": key}; the /api
    // listener fetches the row and assembles the client frame. Backfill does
    // NOT notify (live path only) — catch-up replays would storm listeners,
    // and clients resync by refetch anyway.
    async fn queue_notifies(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> sqlx::Result<()> {
        async fn notify(
            tx: &mut Transaction<'_, Postgres>,
            table: &str,
            key: String,
        ) -> sqlx::Result<()> {
            sqlx::query("SELECT pg_notify('torch_events', $1)")
                .bind(format!("{{\"t\":\"{table}\",\"k\":\"{key}\"}}"))
                .execute(&mut **tx)
                .await?;
            Ok(())
        }
        for r in &self.pools {
            notify(tx, "pools", r.pool_id.to_string()).await?;
        }
        for r in &self.reserves {
            notify(tx, "reserves", r.reserve_id.to_string()).await?;
        }
        for r in &self.swaps {
            notify(tx, "swaps", r.swap_id.to_string()).await?;
        }
        for r in &self.liquidity {
            notify(tx, "liquidity_events", r.liquidity_id.to_string()).await?;
        }
        for r in &self.markets {
            notify(tx, "markets", r.mint.clone()).await?;
        }
        for r in &self.trades {
            notify(tx, "trades", r.trade_id.to_string()).await?;
        }
        for r in &self.messages {
            notify(tx, "messages", r.message_id.to_string()).await?;
        }
        for r in &self.positions {
            notify(
                tx,
                "positions",
                format!("{}|{}|{:?}|{}", r.mint, r.owner, r.side, r.position_index),
            )
            .await?;
        }
        for r in &self.position_events {
            notify(tx, "position_events", r.event_id.to_string()).await?;
        }
        for r in &self.migrations {
            notify(tx, "migrations", r.mint.clone()).await?;
        }
        Ok(())
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
    // [I-5] Seed lp_supply from the DB — an empty cache made every backfilled
    // swap-driven reserves row record lp_supply = 0 (backfill ≠ live).
    let mut lp_supply_cache = load_lp_supply_cache(db).await?;
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
    // [I-1] Chain order: tx position within the block, then inner-ix position.
    // (Sorting by signature reordered same-block transactions arbitrarily —
    // open→close in one block could apply close-first and skip the reconcile.)
    let mut events: Vec<&DecodedEvent> = batch.events.iter().collect();
    events.sort_by(|a, b| {
        a.tx_idx
            .cmp(&b.tx_idx)
            .then(a.inner_ix_idx.cmp(&b.inner_ix_idx))
            .then(a.signature.cmp(&b.signature))
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
        // Live path: notify on commit. (Backfill skips both checkpoint + notify.)
        out.queue_notifies(&mut tx).await?;
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

        // [V21] Leverage — open events upsert a fresh position; close/liquidate
        // reconcile the prior row by delta. Every event also appends to the
        // `position_events` log (idempotent on signature+inner_ix_idx).
        TorchEvent::OpenShort(e) => {
            let row = translate::new_position_open_short(e, de);
            out.positions.push(position::upsert(tx, &row).await?);
            push_position_event(tx, &translate::pos_event_open_short(e, de), out).await?;
        }
        TorchEvent::OpenLong(e) => {
            let row = translate::new_position_open_long(e, de);
            out.positions.push(position::upsert(tx, &row).await?);
            push_position_event(tx, &translate::pos_event_open_long(e, de), out).await?;
        }
        TorchEvent::CloseShort(e) => {
            apply_close_short(tx, e, de, out).await?;
            push_position_event(tx, &translate::pos_event_close_short(e, de), out).await?;
        }
        TorchEvent::CloseLong(e) => {
            apply_close_long(tx, e, de, out).await?;
            push_position_event(tx, &translate::pos_event_close_long(e, de), out).await?;
        }
        TorchEvent::LiquidateShort(e) => {
            apply_liquidate_short(tx, e, de, out).await?;
            push_position_event(tx, &translate::pos_event_liquidate_short(e, de), out).await?;
        }
        TorchEvent::LiquidateLong(e) => {
            apply_liquidate_long(tx, e, de, out).await?;
            push_position_event(tx, &translate::pos_event_liquidate_long(e, de), out).await?;
        }

        // [lifecycle] Status transitions ride program events — the indexer
        // never derives them (single source of truth).
        TorchEvent::BondingCompleted(e) => {
            market::mark_status(
                tx,
                &translate::b58(&e.mint),
                MarketStatus::Complete,
                de.slot,
                now,
            )
            .await?;
        }
        TorchEvent::TokenReclaimed(e) => {
            market::mark_status(
                tx,
                &translate::b58(&e.mint),
                MarketStatus::Reclaimed,
                de.slot,
                now,
            )
            .await?;
        }
        TorchEvent::TokenRevived(e) => {
            // Revival threshold met → trading resumes on the curve.
            market::mark_status(
                tx,
                &translate::b58(&e.mint),
                MarketStatus::Bonding,
                de.slot,
                now,
            )
            .await?;
        }
        TorchEvent::RevivalContribution(_) => {
            // Contributions below the threshold mutate curve SOL only; the
            // reserves snapshot flows through the next BondingCurveTrade.
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

// ───────────── Position (V21) reconcile + event log ─────────────────────
//
// Close*/Liquidate* events carry DELTAS (debt_repaid, tokens_covered, …), not
// absolute amounts. Read the prior `positions` row, apply the diff, upsert. If
// the prior row is missing (event arrived before its OPEN — a backfill-ordering
// edge case), log a warning and skip the current-state reconcile (the
// append-only `position_events` row is still recorded by the caller).
//
// Unit-by-side (mirrors the on-chain generic Position fields):
//   short → collateral = SOL,    debt = tokens
//   long  → collateral = tokens,  debt = SOL

// Append a position_events log row (idempotent on signature+inner_ix_idx).
async fn push_position_event(
    tx: &mut Transaction<'_, Postgres>,
    row: &crate::contracts::NewPositionEventRow,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    if let Some(inserted) = position::event::insert(tx, row).await? {
        out.position_events.push(inserted);
    }
    Ok(())
}

// On-chain interest mirror: `debt × rate × Δslots / (10_000 × epoch_slots)`,
// matching torch_market::math::calc_interest at the indexer's mirrored
// constants (INTEREST_RATE_BPS / EPOCH_DURATION_SLOTS).
fn interest_accrued(debt: i64, last_update_slot: i64, now_slot: i64) -> i64 {
    let slots = now_slot.saturating_sub(last_update_slot).max(0) as u128;
    let debt = debt.max(0) as u128;
    let interest = debt * crate::constants::INTEREST_RATE_BPS * slots
        / (10_000 * crate::constants::EPOCH_DURATION_SLOTS);
    i64::try_from(interest).unwrap_or(i64::MAX)
}

// Build a reconciled `positions` upsert from the prior row — the ON-CHAIN
// mirror ([I-2] remap):
//   debt_amount        -= principal_paid    (NOT the interest-inclusive total)
//   vault_balance      -= vault_delta       (the live held asset; events carry it)
//   collateral_amount   = prior              (on-chain: static record-keeping)
//   accrued_interest    = prior + accrual(prior_debt, Δslots) − interest_paid
// On full resolution everything drains to zero and the row flips inactive.
fn reconcile_position(
    prior: &PositionRow,
    vault_delta: i64,
    principal_paid: i64,
    interest_paid: i64,
    fully_resolved: bool,
    health: PositionHealth,
    de: &DecodedEvent,
) -> NewPositionRow {
    let accrued_at_event = prior.accrued_interest_stored
        + interest_accrued(prior.debt_amount, prior.last_update_slot, de.slot);
    let (debt_amount, vault_balance, accrued, is_active) = if fully_resolved {
        (0, 0, 0, false)
    } else {
        (
            (prior.debt_amount - principal_paid).max(0),
            (prior.vault_balance - vault_delta).max(0),
            (accrued_at_event - interest_paid).max(0),
            true,
        )
    };
    NewPositionRow {
        mint: prior.mint.clone(),
        owner: prior.owner.clone(),
        side: prior.side,
        position_index: prior.position_index,
        collateral_amount: prior.collateral_amount,
        debt_amount,
        open_fee_sol: prior.open_fee_sol,
        vault_balance,
        accrued_interest_stored: accrued,
        last_update_slot: de.slot,
        health,
        is_active,
        owner_is_vault: prior.owner_is_vault,
        created_at: prior.created_at,
        updated_at: translate::ts(de),
    }
}

async fn apply_close_short(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::CloseShortEvent,
    de: &DecodedEvent,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let owner = translate::b58(&e.user);
    let idx = e.position_index as i32;
    let Some(prior) = position::get(tx, &mint, &owner, PositionSide::Short, idx).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %owner, idx, "CloseShort for unknown position; skipping reconcile");
        return Ok(());
    };
    // Vault SOL leaving the position = buyback spend + surplus returned.
    let vault_delta = (e.sol_spent_on_buyback as i64) + (e.surplus_sol_to_user as i64);
    let row = reconcile_position(
        &prior,
        vault_delta,
        e.principal_paid as i64,
        e.interest_paid as i64,
        e.fully_closed,
        PositionHealth::Healthy,
        de,
    );
    out.positions.push(position::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_close_long(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::CloseLongEvent,
    de: &DecodedEvent,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let owner = translate::b58(&e.user);
    let idx = e.position_index as i32;
    let Some(prior) = position::get(tx, &mint, &owner, PositionSide::Long, idx).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %owner, idx, "CloseLong for unknown position; skipping reconcile");
        return Ok(());
    };
    let row = reconcile_position(
        &prior,
        e.tokens_sold as i64, // vault tokens sold off
        e.principal_paid as i64,
        e.interest_paid as i64,
        e.fully_closed,
        PositionHealth::Healthy,
        de,
    );
    out.positions.push(position::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_liquidate_short(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::LiquidateShortEvent,
    de: &DecodedEvent,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let owner = translate::b58(&e.borrower);
    let idx = e.position_index as i32;
    let Some(prior) = position::get(tx, &mint, &owner, PositionSide::Short, idx).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %owner, idx, "LiquidateShort for unknown position; skipping reconcile");
        return Ok(());
    };
    // Vault SOL leaving = seized by liquidator + residual returned to borrower.
    let vault_delta = (e.sol_seized as i64) + (e.residual_sol_to_borrower as i64);
    // The liquidate events carry the interest-inclusive cover; mirror the
    // on-chain split (interest first, then principal).
    let accrued_now = prior.accrued_interest_stored
        + interest_accrued(prior.debt_amount, prior.last_update_slot, de.slot);
    let interest_paid = (e.tokens_covered as i64).min(accrued_now);
    let principal_paid = e.tokens_covered as i64 - interest_paid;
    let row = reconcile_position(
        &prior,
        vault_delta,
        principal_paid,
        interest_paid,
        e.fully_liquidated,
        PositionHealth::Liquidatable,
        de,
    );
    out.positions.push(position::upsert(tx, &row).await?);
    Ok(())
}

async fn apply_liquidate_long(
    tx: &mut Transaction<'_, Postgres>,
    e: &crate::contracts::LiquidateLongEvent,
    de: &DecodedEvent,
    out: &mut WrittenBlock,
) -> anyhow::Result<()> {
    let mint = translate::b58(&e.mint);
    let owner = translate::b58(&e.borrower);
    let idx = e.position_index as i32;
    let Some(prior) = position::get(tx, &mint, &owner, PositionSide::Long, idx).await? else {
        warn!(slot = de.slot, sig = %de.signature, %mint, %owner, idx, "LiquidateLong for unknown position; skipping reconcile");
        return Ok(());
    };
    let accrued_now = prior.accrued_interest_stored
        + interest_accrued(prior.debt_amount, prior.last_update_slot, de.slot);
    let interest_paid = (e.debt_covered as i64).min(accrued_now);
    let principal_paid = e.debt_covered as i64 - interest_paid;
    let row = reconcile_position(
        &prior,
        e.tokens_seized as i64, // vault tokens seized
        principal_paid,
        interest_paid,
        e.fully_liquidated,
        PositionHealth::Liquidatable,
        de,
    );
    out.positions.push(position::upsert(tx, &row).await?);
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
