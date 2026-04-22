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
    let fee: u64 = num.checked_add(9_999)?.checked_div(10_000)?.try_into().ok()?;
    Some(fee.min(MAX_TRANSFER_FEE))
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
