//! Property-based fuzz tests for `torch_market::math`. Each `proptest!` block
//! runs thousands of random inputs; failures auto-shrink to minimal
//! counterexamples. Complements Kani (exhaustive at concrete values) by
//! exploring random inputs across the full u64 range.
//!
//! Located in `tests/` so the `proptest!` macro DSL isn't parsed by anchor's
//! `#[program]` safety-check macro, which walks the lib source tree with syn
//! and doesn't know about macro semantics.
//!
//! Run with `cargo test -p torch_market --test math_proptests`.

use proptest::prelude::*;
use torch_market::constants::*;
use torch_market::math::*;

const CASES: u32 = 5_000;

// Realistic max to keep composite invariants inside u128.
const REALISTIC_MAX: u64 = 1_000_000_000_000_000_000;

// ============================================================================
// Fees
// ============================================================================

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
        let f = calc_transfer_fee(amount).unwrap();
        if f < MAX_TRANSFER_FEE {
            let lhs = (f as u128) * 10_000u128;
            let rhs = (amount as u128) * (TRANSFER_FEE_BPS as u128);
            prop_assert!(lhs >= rhs);
        }
    }
}

// ============================================================================
// Rate curves (treasury decay, creator growth)
// ============================================================================

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

// ============================================================================
// Bonding curve swap
// ============================================================================

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

// ============================================================================
// Lending: collateral value, LTV, interest, liquidation
// ============================================================================

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

// ============================================================================
// Protocol rewards
// ============================================================================

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
        let cap = (distributable as u128 * MAX_CLAIM_SHARE_BPS as u128 / 10_000) as u64;
        prop_assert!(claim <= cap);
        prop_assert!(claim <= distributable);
    }

    #[test]
    fn claim_monopoly_trader_hits_cap(
        distributable in 1_000_000_000u64..100_000_000_000,
        total_vol in 1_000_000_000u64..1_000_000_000_000,
    ) {
        let claim = calc_claim_with_cap(total_vol, distributable, total_vol).unwrap();
        let cap = (distributable as u128 * MAX_CLAIM_SHARE_BPS as u128 / 10_000) as u64;
        prop_assert_eq!(claim, cap);
    }
}

// ============================================================================
// Migration
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    #[test]
    fn tokens_for_pool_cross_multiply(
        real_sol in 1u64..1_000_000_000_000,
        virtual_tokens in 1u64..1_000_000_000_000_000,
        virtual_sol in 1u64..1_000_000_000_000,
    ) {
        // `calc_tokens_for_pool` returns None if the u128 result overflows u64.
        // Treat that as outside the invariant's tested range — we're checking
        // the floor-division property, not overflow behavior.
        let Some(tokens_for_pool) = calc_tokens_for_pool(real_sol, virtual_tokens, virtual_sol) else {
            return Ok(());
        };
        let lhs = (tokens_for_pool as u128) * (virtual_sol as u128);
        let rhs = (real_sol as u128) * (virtual_tokens as u128);
        prop_assert!(lhs <= rhs);
        prop_assert!(rhs - lhs < virtual_sol as u128);
    }
}

// ============================================================================
// Short selling
// ============================================================================

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
        prop_assert!(seized as u128 >= (debt_value as u128) * 10_000 / 10_000);
        let expected_max = (debt_value as u128) * (10_000 + bonus as u128) / 10_000;
        prop_assert!(seized as u128 <= expected_max);
    }
}

// ============================================================================
// Interest accrual lifecycle (re-borrow / re-open no-phantom-interest)
// ============================================================================
//
// Companion to Kani harnesses 71 + 72. The Kani harnesses prove the
// slot-advance post-condition for `apply_interest_accrual` and
// `apply_short_interest_accrual` exhaustively. These proptests validate the
// full call-sequence lifecycle that Kani choked on as a SAT problem:
//
//   open at S0 → fully repaid/closed at R → wait → re-borrow/re-open at B
//   → wait `dormant_window` → accrue at L = B + dormant_window
//
// Property: final accrued interest depends only on `dormant_window`, NOT on
// any earlier slot. The dormant period between R and B (which can be orders
// of magnitude larger than `dormant_window`) must not leak in.
//
// Regression insurance: if a future change to `apply_interest_accrual` or
// `apply_short_interest_accrual` breaks the slot-advance invariant on the
// zero-debt path, these tests fail immediately with a shrunk counterexample.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    #[test]
    fn apply_interest_accrual_re_borrow_no_phantom_interest(
        original_slot in 0u64..1_000_000,
        repay_offset in 1u64..1_000_000,
        reborrow_offset in 1u64..10_000_000,
        dormant_window in 1u64..EPOCH_DURATION_SLOTS,
        new_borrow in MIN_BORROW_AMOUNT..1_000_000_000_000u64,
        rate in 1u16..=DEFAULT_INTEREST_RATE_BPS,
    ) {
        let repay_slot = original_slot + repay_offset;
        let reborrow_slot = repay_slot + reborrow_offset;
        let later_slot = reborrow_slot + dormant_window;

        // Step 1: zero-debt accrue at repay.
        let (acc1, slot1) =
            apply_interest_accrual(0, 0, original_slot, repay_slot, rate).unwrap();
        prop_assert_eq!(slot1, repay_slot);
        prop_assert_eq!(acc1, 0);

        // Step 2: zero-debt accrue at reborrow (after a long dormant gap).
        let (acc2, slot2) =
            apply_interest_accrual(0, 0, slot1, reborrow_slot, rate).unwrap();
        prop_assert_eq!(slot2, reborrow_slot);
        prop_assert_eq!(acc2, 0);

        // Step 3: active-debt accrue at later_slot.
        let (acc3, slot3) =
            apply_interest_accrual(new_borrow, 0, slot2, later_slot, rate).unwrap();
        prop_assert_eq!(slot3, later_slot);

        // Interest must be over `dormant_window`, not (later - original).
        let expected = calc_interest(new_borrow, rate, dormant_window).unwrap();
        prop_assert_eq!(acc3, expected);
    }

    #[test]
    fn apply_short_interest_accrual_re_open_no_phantom_interest(
        original_slot in 0u64..1_000_000,
        close_offset in 1u64..1_000_000,
        reopen_offset in 1u64..10_000_000,
        dormant_window in 1u64..EPOCH_DURATION_SLOTS,
        new_borrow in MIN_SHORT_TOKENS..1_000_000_000_000u64,
        rate in 1u16..=DEFAULT_INTEREST_RATE_BPS,
    ) {
        let close_slot = original_slot + close_offset;
        let reopen_slot = close_slot + reopen_offset;
        let later_slot = reopen_slot + dormant_window;

        let (acc1, slot1) =
            apply_short_interest_accrual(0, 0, original_slot, close_slot, rate).unwrap();
        prop_assert_eq!(slot1, close_slot);
        prop_assert_eq!(acc1, 0);

        let (acc2, slot2) =
            apply_short_interest_accrual(0, 0, slot1, reopen_slot, rate).unwrap();
        prop_assert_eq!(slot2, reopen_slot);
        prop_assert_eq!(acc2, 0);

        let (acc3, slot3) =
            apply_short_interest_accrual(new_borrow, 0, slot2, later_slot, rate).unwrap();
        prop_assert_eq!(slot3, later_slot);

        let expected = calc_short_interest(new_borrow, rate, dormant_window).unwrap();
        prop_assert_eq!(acc3, expected);
    }
}

// ============================================================================
// Math helpers extracted from handlers
//
// Universal-property checks. Concrete-case Kani proofs live alongside the
// helper definitions; these proptests cover the symbolic range that's not
// SAT-tractable in Kani (u128 mul-div with two symbolic operands).
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // apply_bps: result is bounded above by value when bps <= 10_000.
    #[test]
    fn apply_bps_bounded(value in 0u64..REALISTIC_MAX, bps in 0u16..=10_000) {
        let r = apply_bps(value, bps).unwrap();
        prop_assert!(r <= value);
    }

    // apply_bps: monotonic in bps for fixed value.
    #[test]
    fn apply_bps_monotonic_in_bps(value in 0u64..REALISTIC_MAX, a in 0u16..=10_000, b in 0u16..=10_000) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let r_lo = apply_bps(value, lo).unwrap();
        let r_hi = apply_bps(value, hi).unwrap();
        prop_assert!(r_hi >= r_lo);
    }

    // calc_user_borrow_cap: cap = min(formula, absolute), where
    //   formula  = max_lendable × user_collateral × BORROW_SHARE_MULTIPLIER / denominator
    //   absolute = max_lendable × MAX_USER_BORROW_SHARE_BPS / 10_000
    //
    // Since MAX_USER_BORROW_SHARE_BPS = 2000 (20%) and BORROW_SHARE_MULTIPLIER = 23,
    // the absolute cap is always tighter when user_collateral saturates the formula
    // (0.2 × max_lendable < 23 × max_lendable). When user_collateral is small
    // relative to denominator, the formula can be tighter (≈0). Either way, both
    // upper bounds hold.
    #[test]
    fn user_borrow_cap_bounded(
        max_lendable in 0u64..1_000_000_000_000u64, // 1000 SOL
        user_collateral in 0u64..TOTAL_SUPPLY,
        denominator in 1u64..TOTAL_SUPPLY,
    ) {
        let cap = calc_user_borrow_cap(max_lendable, user_collateral, denominator).unwrap();

        // Bound 1: cap is at most the absolute clamp.
        let absolute_cap = apply_bps(max_lendable, MAX_USER_BORROW_SHARE_BPS).unwrap();
        prop_assert!(cap <= absolute_cap);

        // Bound 2: cap is at most the formula value.
        let formula = (max_lendable as u128)
            .checked_mul(user_collateral as u128)
            .unwrap()
            .checked_mul(BORROW_SHARE_MULTIPLIER as u128)
            .unwrap()
            .checked_div(denominator as u128)
            .unwrap();
        prop_assert!((cap as u128) <= formula);
    }

    // calc_user_borrow_cap: zero denominator short-circuits.
    #[test]
    fn user_borrow_cap_zero_denominator(max_lendable in 0u64..u64::MAX, user_collateral in 0u64..u64::MAX) {
        prop_assert_eq!(calc_user_borrow_cap(max_lendable, user_collateral, 0).unwrap(), 0);
    }

    // calc_bad_debt: conservation identity over random valid inputs.
    #[test]
    fn bad_debt_conservation(total_debt in 0u64..REALISTIC_MAX) {
        let debt_to_cover = total_debt / 2; // arbitrary deterministic carve
        let covered = debt_to_cover / 2;
        let bad = calc_bad_debt(total_debt, covered, debt_to_cover).unwrap();
        let uncovered_remainder = total_debt - debt_to_cover;
        prop_assert_eq!(bad + covered + uncovered_remainder, total_debt);
    }

    // calc_price_ratio: monotonic in num for a fixed positive denominator.
    #[test]
    fn price_ratio_monotonic_in_num(a in 0u64..1_000_000_000_000u64, b in 0u64..1_000_000_000_000u64, denom in 1u64..1_000_000_000_000u64) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let r_lo = calc_price_ratio(lo, denom).unwrap();
        let r_hi = calc_price_ratio(hi, denom).unwrap();
        prop_assert!(r_hi >= r_lo);
    }

    // calc_price_ratio: zero denominator returns None.
    #[test]
    fn price_ratio_zero_denom_is_none(num in 0u64..u64::MAX) {
        prop_assert!(calc_price_ratio(num, 0).is_none());
    }

    // calc_short_partial_seize_proration: actual coverage never exceeds the un-capped target.
    #[test]
    fn short_partial_seize_proration_bounded(
        tokens_to_cover in 0u64..1_000_000_000_000u64,
        full_seize in 1u64..100_000_000_000u64,
    ) {
        // capped <= full_seize is the caller invariant; pick a deterministic fraction.
        let capped = full_seize / 2;
        let actual =
            calc_short_partial_seize_proration(tokens_to_cover, capped, full_seize).unwrap();
        prop_assert!(actual <= tokens_to_cover);
    }

    // gross_up_for_transfer_fee: sufficiency + tightness over the full
    // realistic range. Kani proves the same invariant at bounded scale
    // (kani_proofs.rs::verify_gross_up_preserves_net_delivery); proptest
    // covers the wider symbolic range that BMC can't tractably enumerate.
    //
    // Sufficiency: net_received >= net (no protocol underpayment).
    // Tightness:   net_received <= net + 1 (no caller over-payment).
    #[test]
    fn gross_up_preserves_net_delivery(net in 1u64..TOTAL_SUPPLY) {
        let gross = gross_up_for_transfer_fee(net).unwrap();
        let fee = calc_transfer_fee(gross).unwrap();
        let net_received = gross.checked_sub(fee).unwrap();
        prop_assert!(net_received >= net);
        prop_assert!(net_received <= net + 1);
    }
}

// ============================================================================
// [V21] Per-token closed leverage
// ============================================================================

// DeepPool's input-side swap fee, mirrored locally so this integration test
// stays self-contained (deep_pool is a normal dep of the lib, not in scope for
// the test crate). The Kani round-trip proof uses the real
// deep_pool::constants::SWAP_FEE_BPS, pinning correctness against drift.
const DP_SWAP_FEE_BPS: u16 = 25;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // sol_to_token_value is monotonic non-decreasing in the SOL value priced.
    #[test]
    fn sol_to_token_value_monotonic(
        a in 0u64..REALISTIC_MAX,
        b in 0u64..REALISTIC_MAX,
        pool_sol in 1u64..1_000_000_000_000u64,
        pool_tokens in 1_000_000u64..REALISTIC_MAX,
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let (Some(vlo), Some(vhi)) = (
            calc_sol_to_token_value(lo, pool_sol, pool_tokens),
            calc_sol_to_token_value(hi, pool_sol, pool_tokens),
        ) {
            prop_assert!(vhi >= vlo);
        }
    }

    // Round-trip: pricing the bought tokens back into SOL (calc_collateral_value,
    // the inverse) never exceeds the SOL value spent — floor rounding only loses
    // value, never creates it.
    #[test]
    fn sol_to_token_value_roundtrip_bounded(
        sol_value in 0u64..1_000_000_000_000u64,
        pool_sol in 1u64..1_000_000_000_000u64,
        pool_tokens in 1_000_000u64..1_000_000_000_000_000u64,
    ) {
        if let Some(tokens) = calc_sol_to_token_value(sol_value, pool_sol, pool_tokens) {
            if let Some(back) = calc_collateral_value(tokens, pool_sol, pool_tokens) {
                prop_assert!(back <= sol_value);
            }
        }
    }

    // Empty SOL side → None (can't price against a zero reserve).
    #[test]
    fn sol_to_token_value_empty_side_none(sol_value in 0u64..u64::MAX, pool_tokens in 0u64..u64::MAX) {
        prop_assert!(calc_sol_to_token_value(sol_value, 0, pool_tokens).is_none());
    }

    // THE close_short property over the full realistic range: the inverse quote
    // buys enough. Running the returned amount_in through DeepPool's forward buy
    // (mirrored here from deep_pool::math) realizes >= tokens_out. The double-ceil
    // rounding guarantees the close never comes up short of the debt.
    #[test]
    fn close_pool_amount_in_sufficient(
        pool_sol in 5_000_000_000u64..1_000_000_000_000u64,          // 5–1000 SOL
        pool_tokens in 1_000_000_000_000u64..200_000_000_000_000u64, // 1T–200T base units
        frac_bps in 1u64..9_000u64,                                  // order = 0.01%–90% of reserve
    ) {
        let tokens_out = (pool_tokens / 10_000) * frac_bps;
        prop_assume!(tokens_out > 0 && tokens_out < pool_tokens);
        if let Some(amount_in) =
            calc_close_pool_amount_in(tokens_out, pool_sol, pool_tokens, DP_SWAP_FEE_BPS)
        {
            // Forward buy, mirroring deep_pool::math::{calc_swap_fee, calc_swap_output}.
            let fee = (amount_in as u128 * DP_SWAP_FEE_BPS as u128 / 10_000) as u64;
            let effective_in = amount_in - fee;
            let realized = ((effective_in as u128 * pool_tokens as u128)
                / (pool_sol as u128 + effective_in as u128)) as u64;
            prop_assert!(realized >= tokens_out);
        }
    }

    // Guards: zero tokens_out, over-fill (>= reserve), and 100% fee → None.
    #[test]
    fn close_pool_amount_in_none_guards(
        pool_sol in 0u64..1_000_000_000_000u64,
        pool_tokens in 2u64..200_000_000_000_000u64,
        tokens_out in 1u64..u64::MAX,
    ) {
        prop_assert!(calc_close_pool_amount_in(0, pool_sol, pool_tokens, DP_SWAP_FEE_BPS).is_none());
        prop_assert!(
            calc_close_pool_amount_in(pool_tokens, pool_sol, pool_tokens, DP_SWAP_FEE_BPS).is_none()
        );
        // A valid in-range order with a degenerate 100% fee still returns None.
        let valid = tokens_out % (pool_tokens - 1) + 1; // 1 ..= pool_tokens-1
        prop_assert!(calc_close_pool_amount_in(valid, pool_sol, pool_tokens, 10_000).is_none());
    }
}

// ============================================================================
// [V21][D-10] TWAP liquidation pricing — DeepPool-sourced Q64.64 mark
// ============================================================================
//
// The oracle moved into DeepPool (keeperless; docs/twap-oracle.md). Torch now
// consumes a single Q64.64 SOL-per-token price and only keeps the two unit
// conversions (twap_value_in_sol / twap_tokens_to_seize). These fuzz them on the
// new price signature. `price_q64` is built as `(p << 64)` for an integer
// SOL-per-token price p, so the expected values are exact (no fractional
// rounding) and the invariants stay crisp. The ring, accumulation, ratchet, and
// dust floor are DeepPool's and tested there.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // At an integer price p (price_q64 = p<<64), valuing `amount` tokens yields
    // exactly amount*p. Bounds keep the product inside u64 so the floor is exact.
    #[test]
    fn twap_value_integer_price_exact(
        amount in 0u64..100_000_000u64,
        p in 1u64..=1_000u64,
    ) {
        let price_q64 = (p as u128) << 64;
        let want = (amount as u128) * (p as u128);
        prop_assert_eq!(twap_value_in_sol(amount, price_q64), Some(want as u64));
    }

    // Monotone in the token amount at a fixed mark (floor of a linear map).
    // Compare only where both are defined; the larger amount can overflow to None.
    #[test]
    fn twap_value_monotonic_in_amount(
        a in 0u64..REALISTIC_MAX,
        b in 0u64..REALISTIC_MAX,
        price_q64 in 1u128..(1u128 << 96),
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let (Some(vlo), Some(vhi)) = (
            twap_value_in_sol(lo, price_q64),
            twap_value_in_sol(hi, price_q64),
        ) {
            prop_assert!(vhi >= vlo);
        }
    }

    // A zero marked price has no seize basis ⇒ None, regardless of debt/bonus.
    #[test]
    fn twap_seize_zero_price_none(
        debt in 0u64..u64::MAX,
        bonus in 0u16..=10_000u16,
    ) {
        prop_assert!(twap_tokens_to_seize(debt, bonus, 0).is_none());
    }

    // At an integer price p, the seize is exactly debt*(10_000+bonus)/(10_000*p)
    // floored — and the bonus only ever grows it (the at-mark seize clamp).
    #[test]
    fn twap_seize_integer_price_exact_and_bonus_grows(
        debt in 0u64..10_000_000u64,
        bonus in 0u16..=2_000u16,
        p in 1u64..=1_000u64,
    ) {
        let price_q64 = (p as u128) << 64;
        let want = (debt as u128) * (10_000 + bonus as u128) / (10_000u128 * p as u128);
        prop_assert_eq!(twap_tokens_to_seize(debt, bonus, price_q64), Some(want as u64));
        let no_bonus = twap_tokens_to_seize(debt, 0, price_q64).unwrap();
        prop_assert!(want as u64 >= no_bonus);
    }

    // Round-trip: tokens seized for `debt` SOL (no bonus) at an integer price,
    // valued back at that price, recovers ~debt within one price-unit of floor.
    #[test]
    fn twap_seize_value_roundtrip(
        debt in 1u64..10_000_000u64,
        p in 1u64..=1_000u64,
    ) {
        let price_q64 = (p as u128) << 64;
        let tokens = twap_tokens_to_seize(debt, 0, price_q64).unwrap();
        let back = twap_value_in_sol(tokens, price_q64).unwrap();
        // tokens = floor(debt/p); back = tokens*p ≤ debt, and back + p > debt.
        prop_assert!(back <= debt);
        prop_assert!(back + p > debt);
    }

    // Bonus ramp: 0 at/below threshold, full at/above full-LTV, monotone and
    // bounded by max_bonus in between. (effective_liq_bonus_bps stays in torch.)
    #[test]
    fn bonus_ramp_monotone_bounded(
        ltv_a in 0u64..20_000u64,
        ltv_b in 0u64..20_000u64,
    ) {
        let t = DEFAULT_LIQUIDATION_THRESHOLD_BPS;
        let f = LIQ_FULL_BONUS_LTV_BPS;
        let m = DEFAULT_LIQUIDATION_BONUS_BPS;
        let (lo, hi) = if ltv_a <= ltv_b { (ltv_a, ltv_b) } else { (ltv_b, ltv_a) };
        let blo = effective_liq_bonus_bps(lo, t, f, m);
        let bhi = effective_liq_bonus_bps(hi, t, f, m);
        prop_assert!(bhi >= blo);
        prop_assert!(bhi <= m as u64);
        if hi <= t as u64 { prop_assert_eq!(bhi, 0); }
        if lo >= f as u64 { prop_assert_eq!(blo, m as u64); }
    }
}
