//! Pure integer arithmetic for torch_market. No Anchor types, no I/O, no side
//! effects. Every function returns `Option<T>` — `None` means overflow, which
//! handlers surface as `ErrorCode::MathOverflow` via `.ok_or(...)?`.
//!
//! This module is the single source of truth for the arithmetic. Kani proofs
//! in `kani_proofs.rs` import directly from here, so every property proven is
//! proven against the exact code that runs on-chain — not a replica.

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
    Some(
        (sol_received as u128)
            .checked_mul(CREATOR_FEE_SHARE_BPS as u128)?
            .checked_div(10_000)? as u64,
    )
}

// ============================================================================
// Token-2022 transfer fee
// ============================================================================

// Token-2022 transfer fee: ceil-rounded so the withheld amount is never
// below the declared rate, capped at MAX_TRANSFER_FEE.
pub fn calc_transfer_fee(amount: u64) -> Option<u64> {
    let num = (amount as u128).checked_mul(TRANSFER_FEE_BPS as u128)?;
    let fee = num.checked_add(9_999)?.checked_div(10_000)? as u64;
    Some(fee.min(MAX_TRANSFER_FEE))
}

// ============================================================================
// Long lending (borrow SOL against tokens)
// ============================================================================

// Mark-to-market value in SOL of a token collateral balance.
pub fn calc_collateral_value(collateral: u64, pool_sol: u64, pool_tokens: u64) -> Option<u64> {
    Some(
        (collateral as u128)
            .checked_mul(pool_sol as u128)?
            .checked_div(pool_tokens as u128)? as u64,
    )
}

// Loan-to-value in bps. Zero-collateral → u64::MAX (always liquidatable).
pub fn calc_ltv_bps(debt: u64, collateral_value: u64) -> Option<u64> {
    if collateral_value == 0 {
        return Some(u64::MAX);
    }
    Some(
        (debt as u128)
            .checked_mul(10_000)?
            .checked_div(collateral_value as u128)? as u64,
    )
}

// Interest accrual: `principal * rate_bps * slots / (10_000 * epoch_slots)`.
pub fn calc_interest(principal: u64, rate_bps: u16, slots: u64) -> Option<u64> {
    Some(
        (principal as u128)
            .checked_mul(rate_bps as u128)?
            .checked_mul(slots as u128)?
            .checked_div(10_000_u128.checked_mul(EPOCH_DURATION_SLOTS as u128)?)? as u64,
    )
}

// Liquidator's collateral grab on a defaulting long: priced at current pool
// rate, grossed up by `bonus_bps`.
pub fn calc_collateral_to_seize(
    debt: u64,
    bonus_bps: u16,
    pool_tokens: u64,
    pool_sol: u64,
) -> Option<u64> {
    Some(
        (debt as u128)
            .checked_mul((10_000 + bonus_bps as u64) as u128)?
            .checked_mul(pool_tokens as u128)?
            .checked_div(10_000_u128.checked_mul(pool_sol as u128)?)? as u64,
    )
}

// ============================================================================
// Protocol rewards
// ============================================================================

// User's pro-rata share of distributable rewards given their volume.
pub fn calc_user_share(user_vol: u64, distributable: u64, total_vol: u64) -> Option<u64> {
    Some(
        (user_vol as u128)
            .checked_mul(distributable as u128)?
            .checked_div(total_vol as u128)? as u64,
    )
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
    Some(
        (real_sol as u128)
            .checked_mul(virtual_tokens as u128)?
            .checked_div(virtual_sol as u128)? as u64,
    )
}

// ============================================================================
// Short selling (token debt, SOL collateral)
// ============================================================================

// Mark-to-market SOL value of a token debt at current pool rate.
pub fn calc_short_debt_value(token_debt: u64, pool_sol: u64, pool_tokens: u64) -> Option<u64> {
    Some(
        (token_debt as u128)
            .checked_mul(pool_sol as u128)?
            .checked_div(pool_tokens as u128)? as u64,
    )
}

// Short interest in token terms: `tokens_borrowed * rate * slots / (10_000 * epoch_slots)`.
pub fn calc_short_interest(tokens_borrowed: u64, rate_bps: u16, slots: u64) -> Option<u64> {
    Some(
        (tokens_borrowed as u128)
            .checked_mul(rate_bps as u128)?
            .checked_mul(slots as u128)?
            .checked_div(10_000_u128.checked_mul(EPOCH_DURATION_SLOTS as u128)?)? as u64,
    )
}

// SOL to seize on short liquidation: debt value grossed up by bonus.
pub fn calc_short_sol_to_seize(debt_value: u64, bonus_bps: u16) -> Option<u64> {
    Some(
        (debt_value as u128)
            .checked_mul((10_000 + bonus_bps as u64) as u128)?
            .checked_div(10_000)? as u64,
    )
}

#[cfg(test)]
mod proptests {
    //! Property-based fuzz tests covering all 19 pure math fns. Each block
    //! runs thousands of random inputs; failures auto-shrink to minimal
    //! counterexamples. Complements Kani (exhaustive at concrete values)
    //! by exploring random inputs across the full u64 range.
    //!
    //! Run with `cargo test -p torch_market --lib`.

    use super::*;
    use proptest::prelude::*;

    // Per-block case count
    const CASES: u32 = 5_000;

    // Realistic max to keep composite invariants inside u128.
    const REALISTIC_MAX: u64 = 1_000_000_000_000_000_000; // 1e18

    // ========================================================================
    // Fees
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn protocol_fee_bounded(sol in 0u64..u64::MAX / (10_000 / PROTOCOL_FEE_BPS as u64 + 1), bps in 0u16..=10_000) {
            if let Some(f) = calc_protocol_fee(sol, bps) {
                prop_assert!(f <= sol);
            }
        }

        #[test]
        fn protocol_fee_monotonic(a in 0u64..u64::MAX / 10_000, b in 0u64..u64::MAX / 10_000) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let fa = calc_protocol_fee(lo, PROTOCOL_FEE_BPS).unwrap();
            let fb = calc_protocol_fee(hi, PROTOCOL_FEE_BPS).unwrap();
            prop_assert!(fb >= fa);
        }

        #[test]
        fn dev_share_bounded_by_input(total in 0u64..u64::MAX / 10_000) {
            let share = calc_dev_wallet_share(total).unwrap();
            prop_assert!(share <= total);
        }

        #[test]
        fn token_treasury_fee_bounded(sol in 0u64..u64::MAX / 10_000) {
            let f = calc_token_treasury_fee(sol).unwrap();
            prop_assert!(f <= sol);
        }

        #[test]
        fn creator_fee_share_bounded(sol in 0u64..u64::MAX / 10_000) {
            let s = calc_creator_fee_share(sol).unwrap();
            prop_assert!(s <= sol);
        }

        #[test]
        fn transfer_fee_bounded(amount in 0u64..u64::MAX / 10_000) {
            let f = calc_transfer_fee(amount).unwrap();
            prop_assert!(f <= amount.saturating_add(1));
            prop_assert!(f <= MAX_TRANSFER_FEE);
        }

        #[test]
        fn transfer_fee_ceiling(amount in 1u64..u64::MAX / 10_000) {
            // Ceil rounding: fee * 10_000 >= amount * TRANSFER_FEE_BPS
            let f = calc_transfer_fee(amount).unwrap();
            if f < MAX_TRANSFER_FEE {
                let lhs = (f as u128) * 10_000u128;
                let rhs = (amount as u128) * (TRANSFER_FEE_BPS as u128);
                prop_assert!(lhs >= rhs);
            }
        }
    }

    // ========================================================================
    // Rate curves (treasury decay, creator growth)
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn treasury_rate_within_bounds(
            reserves in 0u64..=BONDING_TARGET_TORCH,
            target in prop_oneof![Just(BONDING_TARGET_FLAME), Just(BONDING_TARGET_TORCH)],
        ) {
            let rate = calc_treasury_rate_bps(reserves, target).unwrap();
            prop_assert!(rate >= TREASURY_SOL_MIN_BPS);
            prop_assert!(rate <= TREASURY_SOL_MAX_BPS);
        }

        #[test]
        fn treasury_rate_monotonic_decreasing(
            a in 0u64..=BONDING_TARGET_TORCH,
            b in 0u64..=BONDING_TARGET_TORCH,
            target in prop_oneof![Just(BONDING_TARGET_FLAME), Just(BONDING_TARGET_TORCH)],
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let r_lo = calc_treasury_rate_bps(lo, target).unwrap();
            let r_hi = calc_treasury_rate_bps(hi, target).unwrap();
            // More reserves → less treasury split (rate decays)
            prop_assert!(r_hi <= r_lo);
        }

        #[test]
        fn creator_rate_within_bounds(
            reserves in 0u64..=BONDING_TARGET_TORCH,
            target in prop_oneof![Just(BONDING_TARGET_FLAME), Just(BONDING_TARGET_TORCH)],
        ) {
            let rate = calc_creator_rate_bps(reserves, target).unwrap();
            prop_assert!(rate >= CREATOR_SOL_MIN_BPS);
            prop_assert!(rate <= CREATOR_SOL_MAX_BPS);
        }

        #[test]
        fn creator_rate_monotonic_increasing(
            a in 0u64..=BONDING_TARGET_TORCH,
            b in 0u64..=BONDING_TARGET_TORCH,
            target in prop_oneof![Just(BONDING_TARGET_FLAME), Just(BONDING_TARGET_TORCH)],
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let r_lo = calc_creator_rate_bps(lo, target).unwrap();
            let r_hi = calc_creator_rate_bps(hi, target).unwrap();
            prop_assert!(r_hi >= r_lo);
        }
    }

    // ========================================================================
    // Bonding curve swap
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn tokens_out_bounded_by_vt(
            vt in 1u64..REALISTIC_MAX,
            vs in 1u64..REALISTIC_MAX,
            sol_in in 0u64..REALISTIC_MAX,
        ) {
            let out = calc_tokens_out(vt, vs, sol_in).unwrap();
            prop_assert!(out < vt.saturating_add(1));
            // Tighter: for non-zero input, out < vt (can never drain).
            if sol_in > 0 {
                prop_assert!(out < vt);
            }
        }

        #[test]
        fn tokens_out_zero_input_is_zero(vt in 1u64..REALISTIC_MAX, vs in 1u64..REALISTIC_MAX) {
            prop_assert_eq!(calc_tokens_out(vt, vs, 0).unwrap(), 0);
        }

        #[test]
        fn tokens_out_monotonic(
            vt in 1u64..REALISTIC_MAX,
            vs in 1u64..REALISTIC_MAX,
            a in 0u64..REALISTIC_MAX / 2,
            delta in 0u64..REALISTIC_MAX / 2,
        ) {
            let b = a.saturating_add(delta);
            let oa = calc_tokens_out(vt, vs, a).unwrap();
            let ob = calc_tokens_out(vt, vs, b).unwrap();
            prop_assert!(ob >= oa);
        }

        #[test]
        fn bonding_curve_k_non_decreasing(
            vt in 1_000_000_000u64..100_000_000_000_000,
            vs in 1_000_000_000u64..100_000_000_000_000,
            sol_in in 1u64..10_000_000_000_000,
        ) {
            let out = calc_tokens_out(vt, vs, sol_in).unwrap();
            prop_assume!(out > 0 && out < vt);
            let k_before = (vt as u128) * (vs as u128);
            let k_after = ((vt - out) as u128) * ((vs + sol_in) as u128);
            prop_assert!(k_after >= k_before);
        }

        #[test]
        fn sol_out_bounded_by_vs(
            vt in 1u64..REALISTIC_MAX,
            vs in 1u64..REALISTIC_MAX,
            tokens in 0u64..REALISTIC_MAX,
        ) {
            let out = calc_sol_out(vs, vt, tokens).unwrap();
            if tokens > 0 {
                prop_assert!(out < vs);
            }
        }

        #[test]
        fn sol_out_zero_input_is_zero(vt in 1u64..REALISTIC_MAX, vs in 1u64..REALISTIC_MAX) {
            prop_assert_eq!(calc_sol_out(vs, vt, 0).unwrap(), 0);
        }
    }

    // ========================================================================
    // Lending: collateral value, LTV, interest, liquidation
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn collateral_value_zero_collateral_is_zero(
            pool_sol in 1u64..REALISTIC_MAX,
            pool_tokens in 1u64..REALISTIC_MAX,
        ) {
            prop_assert_eq!(calc_collateral_value(0, pool_sol, pool_tokens).unwrap(), 0);
        }

        #[test]
        fn collateral_value_monotonic_in_collateral(
            a in 0u64..1_000_000_000_000,
            b in 0u64..1_000_000_000_000,
            pool_sol in 1u64..1_000_000_000_000,
            pool_tokens in 1u64..1_000_000_000_000,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let va = calc_collateral_value(lo, pool_sol, pool_tokens).unwrap();
            let vb = calc_collateral_value(hi, pool_sol, pool_tokens).unwrap();
            prop_assert!(vb >= va);
        }

        #[test]
        fn ltv_zero_collateral_is_max(debt in 0u64..REALISTIC_MAX) {
            prop_assert_eq!(calc_ltv_bps(debt, 0).unwrap(), u64::MAX);
        }

        #[test]
        fn ltv_zero_debt_is_zero(collateral in 1u64..REALISTIC_MAX) {
            prop_assert_eq!(calc_ltv_bps(0, collateral).unwrap(), 0);
        }

        #[test]
        fn interest_monotonic_in_principal(
            a in 0u64..100_000_000_000,
            b in 0u64..100_000_000_000,
            rate in 0u16..=10_000,
            slots in 0u64..EPOCH_DURATION_SLOTS * 10,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let ia = calc_interest(lo, rate, slots).unwrap();
            let ib = calc_interest(hi, rate, slots).unwrap();
            prop_assert!(ib >= ia);
        }

        #[test]
        fn interest_monotonic_in_slots(
            principal in 0u64..100_000_000_000,
            rate in 0u16..=10_000,
            a in 0u64..EPOCH_DURATION_SLOTS * 10,
            b in 0u64..EPOCH_DURATION_SLOTS * 10,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let ia = calc_interest(principal, rate, lo).unwrap();
            let ib = calc_interest(principal, rate, hi).unwrap();
            prop_assert!(ib >= ia);
        }

        #[test]
        fn collateral_to_seize_monotonic_in_debt(
            a in 0u64..10_000_000_000,
            b in 0u64..10_000_000_000,
            bonus in 0u16..=5_000,
            pool_sol in 1u64..1_000_000_000_000,
            pool_tokens in 1u64..1_000_000_000_000,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let sa = calc_collateral_to_seize(lo, bonus, pool_tokens, pool_sol).unwrap();
            let sb = calc_collateral_to_seize(hi, bonus, pool_tokens, pool_sol).unwrap();
            prop_assert!(sb >= sa);
        }
    }

    // ========================================================================
    // Protocol rewards
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn user_share_bounded_by_distributable(
            distributable in 0u64..1_000_000_000_000,
            (total_vol, user_vol) in (1u64..1_000_000_000_000).prop_flat_map(|t| (Just(t), 0u64..=t)),
        ) {
            let share = calc_user_share(user_vol, distributable, total_vol).unwrap();
            prop_assert!(share <= distributable);
        }

        #[test]
        fn claim_with_cap_respects_cap(
            distributable in 0u64..1_000_000_000_000,
            (total_vol, user_vol) in (1u64..1_000_000_000_000).prop_flat_map(|t| (Just(t), 0u64..=t)),
        ) {
            let claim = calc_claim_with_cap(user_vol, distributable, total_vol).unwrap();
            // Max is MAX_CLAIM_SHARE_BPS / 10_000 of distributable — i.e. 10%.
            let cap = (distributable as u128 * MAX_CLAIM_SHARE_BPS as u128 / 10_000) as u64;
            prop_assert!(claim <= cap);
            prop_assert!(claim <= distributable);
        }

        #[test]
        fn claim_monopoly_trader_hits_cap(
            distributable in 1_000_000_000u64..100_000_000_000, // 1..100 SOL
            total_vol in 1_000_000_000u64..1_000_000_000_000,   // 1..1000 SOL
        ) {
            // User with 100% of volume gets exactly the cap.
            let claim = calc_claim_with_cap(total_vol, distributable, total_vol).unwrap();
            let cap = (distributable as u128 * MAX_CLAIM_SHARE_BPS as u128 / 10_000) as u64;
            prop_assert_eq!(claim, cap);
        }
    }

    // ========================================================================
    // Migration
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn tokens_for_pool_cross_multiply(
            real_sol in 1u64..1_000_000_000_000,
            virtual_tokens in 1u64..1_000_000_000_000_000,
            virtual_sol in 1u64..1_000_000_000_000,
        ) {
            // Kani proof equivalent: tokens_for_pool * virtual_sol <= real_sol * virtual_tokens,
            // and the shortfall is less than virtual_sol (floor division error).
            let tokens_for_pool = calc_tokens_for_pool(real_sol, virtual_tokens, virtual_sol).unwrap();
            let lhs = (tokens_for_pool as u128) * (virtual_sol as u128);
            let rhs = (real_sol as u128) * (virtual_tokens as u128);
            prop_assert!(lhs <= rhs);
            prop_assert!(rhs - lhs < virtual_sol as u128);
        }
    }

    // ========================================================================
    // Short selling
    // ========================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn short_debt_value_bounded_when_debt_le_reserve(
            pool_sol in 1u64..1_000_000_000_000,
            pool_tokens in 1u64..1_000_000_000_000_000,
            debt_frac in 0u64..=10_000u64,
        ) {
            let token_debt = ((pool_tokens as u128 * debt_frac as u128) / 10_000) as u64;
            let value = calc_short_debt_value(token_debt, pool_sol, pool_tokens).unwrap();
            // Debt up to full token-side of pool can't be worth more than the SOL side
            // (at current pool price).
            prop_assert!(value <= pool_sol);
        }

        #[test]
        fn short_interest_monotonic_in_tokens(
            a in 0u64..100_000_000_000,
            b in 0u64..100_000_000_000,
            rate in 0u16..=10_000,
            slots in 0u64..EPOCH_DURATION_SLOTS * 10,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let ia = calc_short_interest(lo, rate, slots).unwrap();
            let ib = calc_short_interest(hi, rate, slots).unwrap();
            prop_assert!(ib >= ia);
        }

        #[test]
        fn short_sol_to_seize_grossed_up_by_bonus(
            debt_value in 0u64..10_000_000_000,
            bonus in 0u16..=5_000,
        ) {
            let seized = calc_short_sol_to_seize(debt_value, bonus).unwrap();
            // Bonus makes it >= debt_value (within rounding).
            prop_assert!(seized as u128 >= (debt_value as u128) * 10_000 / 10_000);
            // Upper bound: debt + bonus component.
            let expected_max = (debt_value as u128) * (10_000 + bonus as u128) / 10_000;
            prop_assert!(seized as u128 <= expected_max);
        }
    }
}
