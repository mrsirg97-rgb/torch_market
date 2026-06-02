//! Pure integer arithmetic for torch_market. No Anchor types, no I/O, no side
//! effects. Every function returns `Option<T>` — `None` means overflow, which
//! handlers surface as `ErrorCode::MathOverflow` via `.ok_or(...)?`.
//!
//! This module is the single source of truth for the arithmetic. Kani proofs
//! in `kani_proofs.rs` import directly from here, so every property proven is
//! proven against the exact code that runs on-chain — not a replica.
//!
//! Proptests live in `tests/math_proptests.rs` (integration test) so the
//! `proptest!` macro DSL isn't parsed by anchor's `#[program]` safety check.

use crate::constants::*;

// ============================================================================
// Fees & treasury
// ============================================================================

// Protocol fee on SOL inflow/outflow: `sol * fee_bps / 10_000` (floor).
pub fn calc_protocol_fee(sol_amount: u64, fee_bps: u16) -> Option<u64> {
    sol_amount.checked_mul(fee_bps as u64)?.checked_div(10_000)
}

// Dev wallet's slice of the total protocol fee.
pub fn calc_dev_wallet_share(protocol_fee_total: u64) -> Option<u64> {
    protocol_fee_total
        .checked_mul(DEV_WALLET_SHARE_BPS as u64)?
        .checked_div(10_000)
}

// Token treasury's per-buy fee (flat bps of buy SOL).
pub fn calc_token_treasury_fee(sol_amount: u64) -> Option<u64> {
    sol_amount
        .checked_mul(TREASURY_FEE_BPS as u64)?
        .checked_div(10_000)
}

// Decaying treasury split rate: TREASURY_SOL_MAX_BPS at bonding start,
// linearly decaying to TREASURY_SOL_MIN_BPS at target.
pub fn calc_treasury_rate_bps(real_sol_reserves: u64, target: u64) -> Option<u16> {
    let rate_range = (TREASURY_SOL_MAX_BPS - TREASURY_SOL_MIN_BPS) as u128;
    let decay = (real_sol_reserves as u128)
        .checked_mul(rate_range)?
        .checked_div(target as u128)?;
    let rate = (TREASURY_SOL_MAX_BPS as u128).saturating_sub(decay);
    Some(rate.max(TREASURY_SOL_MIN_BPS as u128) as u16)
}

// ============================================================================
// Bonding curve swap
// ============================================================================

// Tokens out for SOL in on a constant-product curve: `vt * sol_in / (vs + sol_in)`.
pub fn calc_tokens_out(vt: u64, vs: u64, sol_in: u64) -> Option<u64> {
    let num = (vt as u128).checked_mul(sol_in as u128)?;
    let den = (vs as u128).checked_add(sol_in as u128)?;
    Some(num.checked_div(den)? as u64)
}

// SOL out for tokens in on a constant-product curve: `vs * tokens / (vt + tokens)`.
pub fn calc_sol_out(vs: u64, vt: u64, tokens: u64) -> Option<u64> {
    let num = (vs as u128).checked_mul(tokens as u128)?;
    let den = (vt as u128).checked_add(tokens as u128)?;
    Some(num.checked_div(den)? as u64)
}

// ============================================================================
// Creator economics
// ============================================================================

// Creator SOL rate, linearly growing from CREATOR_SOL_MIN_BPS at bonding
// start to CREATOR_SOL_MAX_BPS at target.
pub fn calc_creator_rate_bps(real_sol_reserves: u64, target: u64) -> Option<u16> {
    let rate_range = (CREATOR_SOL_MAX_BPS - CREATOR_SOL_MIN_BPS) as u128;
    let growth = (real_sol_reserves as u128)
        .checked_mul(rate_range)?
        .checked_div(target as u128)?;
    let rate = (CREATOR_SOL_MIN_BPS as u128).checked_add(growth)?;
    Some(rate.min(CREATOR_SOL_MAX_BPS as u128) as u16)
}

// Creator's cut of post-migration fee swap proceeds.
pub fn calc_creator_fee_share(sol_received: u64) -> Option<u64> {
    (sol_received as u128)
        .checked_mul(CREATOR_FEE_SHARE_BPS as u128)?
        .checked_div(10_000)?
        .try_into()
        .ok()
}

// ============================================================================
// Token-2022 transfer fee
// ============================================================================

// Token-2022 transfer fee: ceil-rounded so the withheld amount is never
// below the declared rate, capped at MAX_TRANSFER_FEE.
pub fn calc_transfer_fee(amount: u64) -> Option<u64> {
    let num = (amount as u128).checked_mul(TRANSFER_FEE_BPS as u128)?;
    let fee: u64 = num
        .checked_add(9_999)?
        .checked_div(10_000)?
        .try_into()
        .ok()?;
    Some(fee.min(MAX_TRANSFER_FEE))
}

// Gross-up for Token-2022 transfer fee: given a desired NET amount the
// recipient should receive, return the GROSS amount the sender must send.
// Used on short close/liquidate paths where treasury_lock_token_account
// must receive the full debt amount (no depletion) — borrower covers the
// transfer fee on their own repayment.
//
// gross = ceil(net × 10000 / (10000 − TRANSFER_FEE_BPS))
//
// Net received after fee = floor(gross × (10000 − fee_bps) / 10000), so the
// ceiling on the gross ensures the net rounds up to at least `net`.
pub fn gross_up_for_transfer_fee(net: u64) -> Option<u64> {
    let denom = 10_000u128.checked_sub(TRANSFER_FEE_BPS as u128)?;
    if denom == 0 {
        return None;
    }
    let num = (net as u128).checked_mul(10_000)?;
    let gross = num.checked_add(denom - 1)?.checked_div(denom)?;
    gross.try_into().ok()
}

// ============================================================================
// Long lending (borrow SOL against tokens)
// ============================================================================

// Mark-to-market value in SOL of a token collateral balance.
pub fn calc_collateral_value(collateral: u64, pool_sol: u64, pool_tokens: u64) -> Option<u64> {
    (collateral as u128)
        .checked_mul(pool_sol as u128)?
        .checked_div(pool_tokens as u128)?
        .try_into()
        .ok()
}

// Loan-to-value in bps. Zero-collateral → u64::MAX (always liquidatable).
pub fn calc_ltv_bps(debt: u64, collateral_value: u64) -> Option<u64> {
    if collateral_value == 0 {
        return Some(u64::MAX);
    }
    (debt as u128)
        .checked_mul(10_000)?
        .checked_div(collateral_value as u128)?
        .try_into()
        .ok()
}

// Interest accrual: `principal * rate_bps * slots / (10_000 * epoch_slots)`.
pub fn calc_interest(principal: u64, rate_bps: u16, slots: u64) -> Option<u64> {
    (principal as u128)
        .checked_mul(rate_bps as u128)?
        .checked_mul(slots as u128)?
        .checked_div(10_000_u128.checked_mul(EPOCH_DURATION_SLOTS as u128)?)?
        .try_into()
        .ok()
}

// Liquidator's collateral grab on a defaulting long: priced at current pool
// rate, grossed up by `bonus_bps`.
pub fn calc_collateral_to_seize(
    debt: u64,
    bonus_bps: u16,
    pool_tokens: u64,
    pool_sol: u64,
) -> Option<u64> {
    (debt as u128)
        .checked_mul((10_000 + bonus_bps as u64) as u128)?
        .checked_mul(pool_tokens as u128)?
        .checked_div(10_000_u128.checked_mul(pool_sol as u128)?)?
        .try_into()
        .ok()
}

// ============================================================================
// Protocol rewards
// ============================================================================

// User's pro-rata share of distributable rewards given their volume.
pub fn calc_user_share(user_vol: u64, distributable: u64, total_vol: u64) -> Option<u64> {
    (user_vol as u128)
        .checked_mul(distributable as u128)?
        .checked_div(total_vol as u128)?
        .try_into()
        .ok()
}

// Reward claim capped at MAX_CLAIM_SHARE_BPS of distributable per user.
pub fn calc_claim_with_cap(user_vol: u64, distributable: u64, total_vol: u64) -> Option<u64> {
    let share = calc_user_share(user_vol, distributable, total_vol)?;
    let claim_amount = share.min(distributable);
    let max_claim = distributable
        .checked_mul(MAX_CLAIM_SHARE_BPS)?
        .checked_div(10_000)?;
    Some(claim_amount.min(max_claim))
}

// ============================================================================
// Migration
// ============================================================================

// Price-matched migration: token amount seeded into the DEX pool alongside
// the real SOL reserves, preserving bonding-curve price at migration.
pub fn calc_tokens_for_pool(real_sol: u64, virtual_tokens: u64, virtual_sol: u64) -> Option<u64> {
    (real_sol as u128)
        .checked_mul(virtual_tokens as u128)?
        .checked_div(virtual_sol as u128)?
        .try_into()
        .ok()
}

// ============================================================================
// Short selling (token debt, SOL collateral)
// ============================================================================

// Mark-to-market SOL value of a token debt at current pool rate.
pub fn calc_short_debt_value(token_debt: u64, pool_sol: u64, pool_tokens: u64) -> Option<u64> {
    (token_debt as u128)
        .checked_mul(pool_sol as u128)?
        .checked_div(pool_tokens as u128)?
        .try_into()
        .ok()
}

// Short interest in token terms: `tokens_borrowed * rate * slots / (10_000 * epoch_slots)`.
pub fn calc_short_interest(tokens_borrowed: u64, rate_bps: u16, slots: u64) -> Option<u64> {
    (tokens_borrowed as u128)
        .checked_mul(rate_bps as u128)?
        .checked_mul(slots as u128)?
        .checked_div(10_000_u128.checked_mul(EPOCH_DURATION_SLOTS as u128)?)?
        .try_into()
        .ok()
}

// SOL to seize on short liquidation: debt value grossed up by bonus.
pub fn calc_short_sol_to_seize(debt_value: u64, bonus_bps: u16) -> Option<u64> {
    (debt_value as u128)
        .checked_mul((10_000 + bonus_bps as u64) as u128)?
        .checked_div(10_000)?
        .try_into()
        .ok()
}

// ============================================================================
// [V21] Per-token closed leverage
// ============================================================================

// Token amount a given SOL value buys at the current pool ratio:
// `sol_value * pool_tokens / pool_sol` (floor). The inverse-direction mirror of
// `calc_collateral_value` (which prices tokens in SOL). Used to size a short's
// token borrow from its SOL-denominated borrow value — `open_short` step 2
// (D-4). `pool_sol == 0` returns None (can't price against an empty SOL side;
// handlers already gate on positive reserves).
pub fn calc_sol_to_token_value(sol_value: u64, pool_sol: u64, pool_tokens: u64) -> Option<u64> {
    if pool_sol == 0 {
        return None;
    }
    (sol_value as u128)
        .checked_mul(pool_tokens as u128)?
        .checked_div(pool_sol as u128)?
        .try_into()
        .ok()
}

// SOL input required to receive AT LEAST `tokens_out` tokens from a DeepPool
// buy swap, accounting for DeepPool's input-side swap fee. Exact inverse of the
// forward swap (`deep_pool::math::calc_swap_output` after `calc_swap_fee`).
//
//   Forward (buy):  effective_in = amount_in − fee(amount_in)
//                   tokens_out   = effective_in * pool_tokens / (pool_sol + effective_in)
//   Inverse:        effective_in = ceil(pool_sol * tokens_out / (pool_tokens − tokens_out))
//                   amount_in    = ceil(effective_in * 10_000 / (10_000 − swap_fee_bps))
//
// Both steps ceil-round so the realized output is `>= tokens_out` — the close
// must buy enough tokens to repay the (grossed-up) debt. The swap CPI's
// `min_out` is the on-chain backstop if pool state shifts between quote and
// execution. Used by `close_short` (D-5) to compute the SOL pulled from the
// position vault.
//
// `swap_fee_bps` is DeepPool's fee (`deep_pool::constants::SWAP_FEE_BPS`), taken
// as a parameter so this stays pure and the proofs can vary it. Returns None if
// `tokens_out` is 0 or `>= pool_tokens` (can't drain/over-fill the pool),
// `swap_fee_bps >= 10_000`, or on overflow.
pub fn calc_close_pool_amount_in(
    tokens_out: u64,
    pool_sol: u64,
    pool_tokens: u64,
    swap_fee_bps: u16,
) -> Option<u64> {
    if tokens_out == 0 || tokens_out >= pool_tokens {
        return None;
    }
    let fee_denom = 10_000u128.checked_sub(swap_fee_bps as u128)?;
    if fee_denom == 0 {
        return None;
    }
    // effective_in = ceil(pool_sol * tokens_out / (pool_tokens − tokens_out))
    let num = (pool_sol as u128).checked_mul(tokens_out as u128)?;
    let den = (pool_tokens as u128).checked_sub(tokens_out as u128)?; // > 0 by the guard above
    let effective_in = num.checked_add(den.checked_sub(1)?)?.checked_div(den)?;
    // amount_in = ceil(effective_in * 10_000 / (10_000 − swap_fee_bps))
    let amount_in = effective_in
        .checked_mul(10_000)?
        .checked_add(fee_denom.checked_sub(1)?)?
        .checked_div(fee_denom)?;
    amount_in.try_into().ok()
}

// ============================================================================
// Interest accrual state transition
// ============================================================================
//
// Pure state-transition for `accrue_interest`. Returns the new
// `(accrued_interest, last_update_slot)` pair. Extracted from the handlers so
// the post-condition is Kani-verifiable: see `verify_interest_accrual_*`.
//
// Post-condition: when `Some(_)` is returned, the second element equals
// `current_slot` — regardless of whether interest was accrued. This is the
// invariant that prevents stale-slot bugs on re-borrow of a position that was
// fully repaid but not closed.
//
// Returns `None` on arithmetic overflow.

pub fn apply_interest_accrual(
    borrowed: u64,
    accrued: u64,
    last_slot: u64,
    current_slot: u64,
    rate_bps: u16,
) -> Option<(u64, u64)> {
    if borrowed == 0 {
        // No active debt — advance the slot anyway so a future re-borrow on
        // this same account doesn't pick up the dormant period as interest.
        return Some((accrued, current_slot));
    }
    let slots_elapsed = current_slot.saturating_sub(last_slot);
    if slots_elapsed == 0 {
        return Some((accrued, current_slot));
    }
    let interest = calc_interest(borrowed, rate_bps, slots_elapsed)?;
    let new_accrued = accrued.checked_add(interest)?;
    Some((new_accrued, current_slot))
}

// Same shape, token-debt arithmetic (uses `calc_short_interest`).
pub fn apply_short_interest_accrual(
    tokens_borrowed: u64,
    accrued: u64,
    last_slot: u64,
    current_slot: u64,
    rate_bps: u16,
) -> Option<(u64, u64)> {
    if tokens_borrowed == 0 {
        return Some((accrued, current_slot));
    }
    let slots_elapsed = current_slot.saturating_sub(last_slot);
    if slots_elapsed == 0 {
        return Some((accrued, current_slot));
    }
    let interest = calc_short_interest(tokens_borrowed, rate_bps, slots_elapsed)?;
    let new_accrued = accrued.checked_add(interest)?;
    Some((new_accrued, current_slot))
}

// ============================================================================
// Generic bps helpers
// ============================================================================

// `value × bps / 10_000` with u128 intermediate so the multiplication never
// overflows for any u64 × u16 pair. Floor-rounded. Used everywhere a flat or
// dynamic basis-point fee/split/cap is applied.
pub fn apply_bps(value: u64, bps: u16) -> Option<u64> {
    (value as u128)
        .checked_mul(bps as u128)?
        .checked_div(10_000)?
        .try_into()
        .ok()
}

// ============================================================================
// Lending / short caps
// ============================================================================

// Per-user borrow cap: `max_lendable × user_collateral × BORROW_SHARE_MULTIPLIER
// / denominator`, clamped to `max_lendable × MAX_USER_BORROW_SHARE_BPS / 10000`.
//
// The denominator is what each caller treats as "everyone's collateral pool":
//   - Long lending  → TOTAL_SUPPLY (collateral is tokens; cap each user against
//     a share of total supply).
//   - Short selling → derived treasury SOL (treasury_sol_vault lamports; collateral
//     is SOL; cap each user against a share of treasury SOL).
//
// The clamp is the load-bearing safety: without it, a user with > ~4.35% of
// total supply as collateral could take the entire lendable amount, since
// BORROW_SHARE_MULTIPLIER × user_share crosses 1.0. The clamp makes the
// per-user cap actually mean "at most N% of lendable per user" regardless
// of collateral size.
//
// `denominator == 0` is the no-pool case and returns 0 (no borrow allowed).
pub fn calc_user_borrow_cap(
    max_lendable: u64,
    user_collateral: u64,
    denominator: u64,
) -> Option<u64> {
    if denominator == 0 {
        return Some(0);
    }
    let formula_cap: u64 = (max_lendable as u128)
        .checked_mul(user_collateral as u128)?
        .checked_mul(BORROW_SHARE_MULTIPLIER as u128)?
        .checked_div(denominator as u128)?
        .try_into()
        .ok()?;
    let absolute_cap = apply_bps(max_lendable, MAX_USER_BORROW_SHARE_BPS)?;
    Some(formula_cap.min(absolute_cap))
}

// ============================================================================
// Liquidation accounting
// ============================================================================

// Bad debt left on the position after a (possibly partial) liquidation.
//
//   bad_debt = total_debt − (covered + (total_debt − debt_to_cover))
//
// Algebraic identity: when `debt_to_cover == total_debt`, this is `total_debt
// − covered`. When `debt_to_cover < total_debt`, the uncovered portion stays
// on the position so `bad_debt == debt_to_cover − covered`.
//
// Both inner subtractions are invariant-safe by construction:
// `debt_to_cover <= total_debt` and `covered <= debt_to_cover`. checked_sub is
// used to fail loud if any caller violates that invariant.
pub fn calc_bad_debt(total_debt: u64, covered: u64, debt_to_cover: u64) -> Option<u64> {
    let uncovered_remainder = total_debt.checked_sub(debt_to_cover)?;
    let total_resolved = covered.checked_add(uncovered_remainder)?;
    total_debt.checked_sub(total_resolved)
}

// ============================================================================
// Pricing / ratios
// ============================================================================

// Token price ratio in `RATIO_PRECISION`-scaled fixed point: `num × precision
// / denom`. `denom == 0` returns `None` (cannot price against an empty side).
pub fn calc_price_ratio(num: u64, denom: u64) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    (num as u128)
        .checked_mul(RATIO_PRECISION)?
        .checked_div(denom as u128)?
        .try_into()
        .ok()
}

// ============================================================================
// Short-specific
// ============================================================================

// When a short liquidation would seize more SOL than the position holds, the
// covered token debt is prorated by the ratio of actual seizable collateral
// to the would-be seizure. This is the short equivalent of the long's
// `calc_collateral_value(actual_collateral_seized, ...)` rescue path.
//
//   actual_tokens_covered = tokens_to_cover × capped_collateral / full_seize
//
// `full_seize == 0` is undefined (would mean the liquidator wants to cover
// debt with no SOL outflow) and returns `None`.
pub fn calc_short_partial_seize_proration(
    tokens_to_cover: u64,
    capped_collateral: u64,
    full_seize: u64,
) -> Option<u64> {
    if full_seize == 0 {
        return None;
    }
    (tokens_to_cover as u128)
        .checked_mul(capped_collateral as u128)?
        .checked_div(full_seize as u128)?
        .try_into()
        .ok()
}

// ============================================================================
// [V21][D-10] TWAP liquidation pricing — DeepPool-sourced Q64.64 mark
// ============================================================================
//
// The liquidation TRIGGER (LTV) and SEIZE accounting are marked against a
// time-weighted average price, never raw spot — this is what closes the
// spot-AMM-as-oracle hole (docs/v21-closed-loop-leverage.md §D-10). DeepPool is a
// generic CPMM with no oracle, so its instantaneous `pool_sol / pool_tokens` is a
// single-tx-movable price; a TWAP makes the mark something an attacker must HOLD
// off-market across a window, not snipe.
//
// As of v21 the oracle ITSELF lives in DeepPool (keeperless — a CPMM's price
// moves only on swaps, so the pool accumulates on the swap; there is nothing to
// sample between swaps, hence no torch crank). Torch is a pure consumer: at
// liquidation it deserializes the `deep_pool::Pool` already in the context and
// calls `Pool::read_twap_sol_per_tok`, which returns a single Q64.64 price =
// time-weighted SOL-per-token (64 fractional bits) over torch's chosen lookback.
// The two helpers below are the ONLY TWAP surface left in torch — they convert
// that Q64.64 mark into a SOL value / a token seize amount, the at-mark mirrors
// of `calc_collateral_value` / `calc_collateral_to_seize` (which price raw spot).
// All ring storage, accumulation, warmup, and the dust-liquidity floor live in
// DeepPool now (docs/twap-oracle.md); none of it is torch's concern.

// Time-weighted SOL value of `token_amount` at the TWAP mark:
//   `token_amount × price_q64 ÷ 2^64` (floor).
// Used for BOTH the long collateral mark (value the vault tokens) and the short
// debt mark (value the token debt) — both are "price N tokens in SOL at the
// mark". Overflow in the widening multiply → None → the caller fails closed
// (refuses to liquidate); only reachable at token amounts far beyond any real
// pool's supply. A warmup / too-young pool is signalled upstream by
// `read_twap_sol_per_tok` returning `None`, never here.
pub fn twap_value_in_sol(token_amount: u64, price_q64: u128) -> Option<u64> {
    let scaled = (token_amount as u128).checked_mul(price_q64)?;
    (scaled >> 64).try_into().ok()
}

// Token amount a liquidator seizes to cover `debt_sol` of a long's SOL debt,
// priced at the TWAP mark and grossed up by `bonus_bps`:
//   `debt_sol × (10_000 + bonus_bps) × 2^64 ÷ (10_000 × price_q64)` (floor).
// The at-mark mirror of `calc_collateral_to_seize` (which uses raw spot
// `pool_tokens / pool_sol`). Pricing the seize at the mark — not spot — stops an
// attacker pumping/dumping spot at liquidation time to over-seize a genuinely
// liquidatable position (D-10 "seize clamp"). `price_q64 == 0` (a vanishingly
// small marked price) → None: no seize basis, fail closed. Overflow in the
// grossed-up widening multiply → None — only reachable for a single position
// whose SOL debt exceeds ~1.1M SOL, far beyond any pool's depth.
pub fn twap_tokens_to_seize(debt_sol: u64, bonus_bps: u16, price_q64: u128) -> Option<u64> {
    if price_q64 == 0 {
        return None;
    }
    let grossed = (debt_sol as u128).checked_mul(10_000 + bonus_bps as u128)?;
    let numerator = grossed.checked_mul(1u128 << 64)?;
    let denom = price_q64.checked_mul(10_000)?;
    numerator.checked_div(denom)?.try_into().ok()
}

// Distress-scaled liquidation bonus (D-10 manipulation-prize cap). The bonus
// ramps linearly 0 → `max_bonus_bps` as the (TWAP-marked) LTV rises from
// `threshold_bps` to `full_bonus_ltv_bps`:
//
//   ltv ≤ threshold       → 0           (not liquidatable; no prize)
//   ltv ≥ full_bonus_ltv  → max_bonus   (genuine deep distress; full prize)
//   between               → max_bonus × (ltv − threshold) / (full − threshold)
//
// A manufactured liquidation can only shove a position JUST past the threshold
// (the ratchet TWAP makes a bigger push a slow, expensive hold — D-10), where the
// bonus is ~0, so there's no prize to repay the manipulation cost. A genuine deep
// liquidation still pays the full bonus to honest liquidators.
//
// Returns a plain u64 (not Option): the result is bounded to `[0, max_bonus_bps]`
// and cannot overflow. `ltv_bps` may be u64::MAX (zero-collateral, from
// `calc_ltv_bps`) — the `>= full` guard short-circuits before any multiply, so
// the product only runs for `ltv < full_bonus_ltv_bps` (< ~9000), keeping
// `max_bonus × (ltv − threshold)` far inside u64. `full ≤ threshold` (misconfig)
// → 0. This is the one math fn that escapes the module's Option convention
// precisely because no input can make it fail.
pub fn effective_liq_bonus_bps(
    ltv_bps: u64,
    threshold_bps: u16,
    full_bonus_ltv_bps: u16,
    max_bonus_bps: u16,
) -> u64 {
    if ltv_bps <= threshold_bps as u64 {
        return 0;
    }
    if ltv_bps >= full_bonus_ltv_bps as u64 {
        return max_bonus_bps as u64;
    }
    let span = (full_bonus_ltv_bps as u64).saturating_sub(threshold_bps as u64);
    if span == 0 {
        return 0;
    }
    (max_bonus_bps as u64).saturating_mul(ltv_bps - threshold_bps as u64) / span
}
