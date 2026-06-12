// PnLService — per-wallet realized PnL via FIFO cost basis over the
// wallet's full trade history (bonding-curve `trades` ∪ DEX `swaps`).
//
// FIFO is the standard accounting model: each sell is matched against the
// oldest unsold buys to compute cost basis. Realized PnL is the difference
// between SOL received and cost basis consumed.
//
// What this CAN'T detect (acceptable limitations for a v1):
//  - Tokens received via direct transfer (no buy event) → selling them
//    shows as 100% profit (no cost basis to consume).
//  - Tokens sent out via direct transfer (no sell event) → cost basis
//    stays on the books, under-counts realized PnL.
//  - Margin positions: SOL-denominated outcomes ARE folded in (see the
//    position_events pass below) — collateral in vs surplus/residual out,
//    counted on RESOLUTION (open positions are the unrealized side, shown
//    live elsewhere). Long token legs (token collateral out / vault tokens
//    back) still bypass FIFO inventory — accepted drift, same class as
//    direct transfers.
//
// Net effect: PnL is an approximation based on swap activity through the
// protocol. Power users with significant off-protocol token movement will
// see some drift; typical traders see accurate numbers.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::Row;

use crate::services::context::RequestCtx;

#[derive(Debug, Clone, Serialize)]
pub struct UserPnlByMint {
    pub mint: String,
    pub tokens_remaining: i64,
    pub cost_basis_remaining: i64, // lamports spent on remaining tokens
    pub realized_pnl: i64,         // lamports (SOL received - cost basis sold), can be negative
    pub total_buy_volume: i64,     // total lamports spent buying
    pub total_sell_volume: i64,    // total lamports received from sells
    pub trade_count: i64,
    // [2026-06-12] SOL realized from RESOLVED margin positions on this mint:
    // Σ(close surplus + liquidation residual) − Σ(open SOL collateral).
    // Included in realized_pnl; broken out for transparency.
    pub position_pnl: i64,
    pub position_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserPnlSummary {
    pub wallet: String,
    pub by_mint: Vec<UserPnlByMint>,
    pub total_realized_pnl: i64,
    pub total_volume: i64, // buy_volume + sell_volume aggregated
    pub total_trade_count: i64,
}

pub struct PnlService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> PnlService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    /// Compute realized PnL for `wallet`, aggregated per mint.
    /// `vault`: the wallet's torch_vault PDA (client-derived) — vault-routed
    /// DEX swaps attribute to the vault pubkey; wallet + vault are one
    /// economic actor. Pass the wallet itself when absent (harmless no-op).
    pub async fn for_wallet(&mut self, wallet: &str, vault: &str) -> sqlx::Result<UserPnlSummary> {
        // Single query: union the wallet's bonding-curve trades + DEX swaps,
        // normalized into a (mint, is_buy, sol_amount, token_amount)
        // representation, ordered by mint then by time so we can FIFO each
        // mint's stream in one pass.
        let rows = sqlx::query(
            "WITH user_events AS (
                SELECT
                    t.mint,
                    t.is_buy,
                    (CASE WHEN t.is_buy THEN t.sol_in ELSE t.sol_out END)::bigint AS sol_amount,
                    (CASE WHEN t.is_buy THEN t.tokens_out ELSE t.tokens_in END)::bigint AS token_amount,
                    t.created_at
                FROM trades t
                WHERE t.trader IN ($1, $2)
                UNION ALL
                SELECT
                    p.token_mint AS mint,
                    s.is_buy,
                    (CASE WHEN s.is_buy THEN s.amount_in_net ELSE s.amount_out_net END)::bigint AS sol_amount,
                    (CASE WHEN s.is_buy THEN s.amount_out_net ELSE s.amount_in_net END)::bigint AS token_amount,
                    s.created_at
                FROM swaps s
                JOIN pools p ON s.pool_id = p.pool_id
                WHERE s.user_pk IN ($1, $2)
            )
            SELECT mint, is_buy, sol_amount, token_amount
            FROM user_events
            ORDER BY mint ASC, created_at ASC",
        )
        .bind(wallet)
        .bind(vault)
        .fetch_all(&mut *self.ctx.tx)
        .await?;

        let mut by_mint: HashMap<String, MintAccumulator> = HashMap::new();

        for row in rows {
            let mint: String = row.get("mint");
            let is_buy: bool = row.get("is_buy");
            let sol_amount: i64 = row.get("sol_amount");
            let token_amount: i64 = row.get("token_amount");

            let state = by_mint.entry(mint).or_default();
            state.trade_count += 1;
            if is_buy {
                state.total_buy_volume = state.total_buy_volume.saturating_add(sol_amount);
                state.inventory.push((token_amount, sol_amount));
            } else {
                state.total_sell_volume = state.total_sell_volume.saturating_add(sol_amount);
                // Consume FIFO inventory to determine cost basis for this sell.
                let mut tokens_to_sell = token_amount;
                let mut cost_basis_used: i64 = 0;
                while tokens_to_sell > 0 && !state.inventory.is_empty() {
                    let (head_tokens, head_sol) = state.inventory[0];
                    if head_tokens <= tokens_to_sell {
                        cost_basis_used = cost_basis_used.saturating_add(head_sol);
                        tokens_to_sell -= head_tokens;
                        state.inventory.remove(0);
                    } else {
                        // Partial consume — split the batch proportionally
                        let cost_portion = (head_sol as i128 * tokens_to_sell as i128
                            / head_tokens as i128)
                            as i64;
                        cost_basis_used = cost_basis_used.saturating_add(cost_portion);
                        state.inventory[0] =
                            (head_tokens - tokens_to_sell, head_sol - cost_portion);
                        tokens_to_sell = 0;
                    }
                }
                state.realized_pnl = state
                    .realized_pnl
                    .saturating_add(sol_amount.saturating_sub(cost_basis_used));
                // If tokens_to_sell > 0 here, the wallet sold more than it
                // bought through the protocol (received via transfer).
                // Treat as cost basis 0 (full sale price = realized PnL).
            }
        }

        // ── Margin position outcomes (the +2-SOL-realized-0 bug, 2026-06-12):
        // closes pay out via position_events, invisible to trade-FIFO. Fold
        // SOL flows per position; count only RESOLVED positions into realized.
        let pos_rows = sqlx::query(
            "SELECT mint, side::text AS side, position_index, kind::text AS kind,
                    COALESCE(sol_in, 0)::bigint AS sol_in,
                    COALESCE(surplus_sol, 0)::bigint AS surplus_sol,
                    COALESCE(residual, 0)::bigint AS residual,
                    COALESCE(fully_resolved, false) AS fully_resolved
             FROM position_events
             WHERE owner IN ($1, $2)
             ORDER BY mint, side, position_index, event_id",
        )
        .bind(wallet)
        .bind(vault)
        .fetch_all(&mut *self.ctx.tx)
        .await?;

        #[derive(Default)]
        struct PosAcc {
            sol_in: i64,
            sol_out: i64,
            resolved: bool,
        }
        let mut positions: HashMap<(String, String, i32), PosAcc> = HashMap::new();
        for row in pos_rows {
            let mint: String = row.get("mint");
            let side: String = row.get("side");
            let idx: i32 = row.get("position_index");
            let kind: String = row.get("kind");
            let acc = positions.entry((mint, side.clone(), idx)).or_default();
            match kind.as_str() {
                // Short opens stake SOL collateral (sol_in = gross collateral).
                // Long opens stake TOKEN collateral — no wallet SOL outflow.
                "open" if side == "short" => acc.sol_in = acc.sol_in.saturating_add(row.get::<i64, _>("sol_in")),
                "close" => {
                    acc.sol_out = acc.sol_out.saturating_add(row.get::<i64, _>("surplus_sol"));
                    if row.get::<bool, _>("fully_resolved") {
                        acc.resolved = true;
                    }
                }
                "liquidate" => {
                    acc.sol_out = acc.sol_out.saturating_add(row.get::<i64, _>("residual"));
                    if row.get::<bool, _>("fully_resolved") {
                        acc.resolved = true;
                    }
                }
                _ => {}
            }
        }
        for ((mint, _side, _idx), acc) in positions {
            if !acc.resolved {
                continue; // open position = unrealized; shown live elsewhere
            }
            let state = by_mint.entry(mint).or_default();
            let pnl = acc.sol_out.saturating_sub(acc.sol_in);
            state.position_pnl = state.position_pnl.saturating_add(pnl);
            state.position_count += 1;
            state.realized_pnl = state.realized_pnl.saturating_add(pnl);
        }

        let mut by_mint_vec: Vec<UserPnlByMint> = by_mint
            .into_iter()
            .map(|(mint, acc)| {
                let tokens_remaining: i64 = acc.inventory.iter().map(|(t, _)| *t).sum();
                let cost_basis_remaining: i64 = acc.inventory.iter().map(|(_, s)| *s).sum();
                UserPnlByMint {
                    mint,
                    tokens_remaining,
                    cost_basis_remaining,
                    realized_pnl: acc.realized_pnl,
                    total_buy_volume: acc.total_buy_volume,
                    total_sell_volume: acc.total_sell_volume,
                    trade_count: acc.trade_count,
                    position_pnl: acc.position_pnl,
                    position_count: acc.position_count,
                }
            })
            .collect();

        // Sort by absolute realized PnL descending — most-impactful mints
        // surface first in the UI.
        by_mint_vec.sort_by_key(|e| -e.realized_pnl.abs());

        let total_realized_pnl: i64 = by_mint_vec.iter().map(|e| e.realized_pnl).sum();
        let total_volume: i64 = by_mint_vec
            .iter()
            .map(|e| e.total_buy_volume.saturating_add(e.total_sell_volume))
            .sum();
        let total_trade_count: i64 = by_mint_vec.iter().map(|e| e.trade_count).sum();

        Ok(UserPnlSummary {
            wallet: wallet.to_string(),
            by_mint: by_mint_vec,
            total_realized_pnl,
            total_volume,
            total_trade_count,
        })
    }
}

// Per-mint accumulator state during FIFO walk. Inventory holds (tokens,
// remaining_sol_cost) pairs in arrival order — push to end on buy,
// drain from front on sell.
#[derive(Default)]
struct MintAccumulator {
    inventory: Vec<(i64, i64)>,
    realized_pnl: i64,
    total_buy_volume: i64,
    total_sell_volume: i64,
    trade_count: i64,
    position_pnl: i64,
    position_count: i64,
}
