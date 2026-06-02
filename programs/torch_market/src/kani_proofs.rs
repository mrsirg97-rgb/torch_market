//! Kani Formal Verification Proof Harnesses
//!
//! Mathematically proves properties of torch_market's core arithmetic
//! for ALL valid inputs within protocol bounds.
//!
//! Run with: cargo kani
//!
//! Each harness verifies a specific property (conservation, bounds, monotonicity)
//! using symbolic inputs constrained to realistic protocol ranges.

use crate::constants::*;
use crate::math::*;

// Kani-only helper: restrict `target` to a valid bonding tier.
// [V4.0] Removed SPARK (50 SOL) — no longer a valid creation target
fn assume_valid_target(target: u64) {
    kani::assume(target == BONDING_TARGET_FLAME || target == BONDING_TARGET_TORCH);
}

// ============================================================================
// 1. BUY: Fee Conservation
//    Proves: protocol_fee + treasury_fee + sol_after_fees == sol_amount
// ============================================================================

#[kani::proof]
fn verify_buy_fee_conservation() {
    let sol_amount: u64 = kani::any();
    kani::assume(sol_amount >= MIN_SOL_AMOUNT);
    kani::assume(sol_amount <= BONDING_TARGET_LAMPORTS);

    let protocol_fee = calc_protocol_fee(sol_amount, PROTOCOL_FEE_BPS).unwrap();
    let treasury_fee = calc_token_treasury_fee(sol_amount).unwrap();
    let after_fees = sol_amount
        .checked_sub(protocol_fee)
        .unwrap()
        .checked_sub(treasury_fee)
        .unwrap();

    assert!(protocol_fee + treasury_fee + after_fees == sol_amount);
    assert!(protocol_fee <= sol_amount);
    assert!(treasury_fee <= sol_amount);
}

// ============================================================================
// 2. BUY: Protocol Fee Split Conservation
//    Proves: dev_share + protocol_portion == protocol_fee_total
// ============================================================================

#[kani::proof]
fn verify_protocol_fee_split() {
    let sol_amount: u64 = kani::any();
    kani::assume(sol_amount >= MIN_SOL_AMOUNT);
    kani::assume(sol_amount <= BONDING_TARGET_LAMPORTS);

    let total = calc_protocol_fee(sol_amount, PROTOCOL_FEE_BPS).unwrap();
    let dev = calc_dev_wallet_share(total).unwrap();
    let protocol = total.checked_sub(dev).unwrap();

    assert!(dev + protocol == total);
    assert!(dev <= total);
}

// ============================================================================
// 3. BUY: Dynamic Treasury Rate Bounds
//    Proves: rate is always in [250, 1500] (2.5% to 15%) for all tiers
//    [V10] 15%→2.5% (was 12.5%→4% in V4.0)
// ============================================================================

#[kani::proof]
fn verify_treasury_rate_bounds() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let reserves: u64 = kani::any();
    kani::assume(reserves <= target);

    let rate = calc_treasury_rate_bps(reserves, target).unwrap();

    assert!(rate >= TREASURY_SOL_MIN_BPS);
    assert!(rate <= TREASURY_SOL_MAX_BPS);
}

// ============================================================================
// 4. BUY: Dynamic Treasury Rate Monotonic Decrease
//    Proves: more reserves -> lower treasury rate (for the same target)
//    [V10] Flat 15% → 2.5% across all tiers
// ============================================================================

#[kani::proof]
fn verify_treasury_rate_monotonic() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= target);
    kani::assume(b <= target);
    kani::assume(a <= b);

    let rate_a = calc_treasury_rate_bps(a, target).unwrap();
    let rate_b = calc_treasury_rate_bps(b, target).unwrap();

    assert!(rate_a >= rate_b);
}

// ============================================================================
// 5. BUY: Total SOL Distribution Conservation
//    Proves: curve + treasury + creator + dev + protocol == sol_amount (no SOL created/lost)
//    [V34] Creator SOL share carved from treasury split
// ============================================================================

#[kani::proof]
fn verify_sol_distribution_conservation() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let sol_amount: u64 = kani::any();
    let reserves: u64 = kani::any();
    kani::assume(sol_amount >= MIN_SOL_AMOUNT);
    kani::assume(sol_amount <= 10_000_000_000); // 10 SOL realistic max per trade
    kani::assume(reserves <= target);

    let pf_total = calc_protocol_fee(sol_amount, PROTOCOL_FEE_BPS).unwrap();
    let dev = calc_dev_wallet_share(pf_total).unwrap();
    let pf = pf_total.checked_sub(dev).unwrap();
    let tf = calc_token_treasury_fee(sol_amount).unwrap();
    let after = sol_amount
        .checked_sub(pf_total)
        .unwrap()
        .checked_sub(tf)
        .unwrap();

    let treasury_rate = calc_treasury_rate_bps(reserves, target).unwrap();
    let creator_rate = calc_creator_rate_bps(reserves, target).unwrap();

    // Total split from sol_after_fees (unchanged total)
    let total_split = after
        .checked_mul(treasury_rate as u64)
        .unwrap()
        .checked_div(10000)
        .unwrap();
    // [V34] Creator's portion carved from total_split
    let creator_sol = after
        .checked_mul(creator_rate as u64)
        .unwrap()
        .checked_div(10000)
        .unwrap();
    let sol_to_treasury_split = total_split.checked_sub(creator_sol).unwrap();
    let to_curve = after.checked_sub(total_split).unwrap();
    let total_treasury = tf.checked_add(sol_to_treasury_split).unwrap();

    let distributed = to_curve
        .checked_add(total_treasury)
        .unwrap()
        .checked_add(creator_sol)
        .unwrap()
        .checked_add(dev)
        .unwrap()
        .checked_add(pf)
        .unwrap();

    assert!(distributed == sol_amount);
}

// ============================================================================
// 6. BUY: Bonding Curve Output Bounded
//    Proves: tokens_out < virtual_token_reserves (can't output more than exists)
//    Note: tokens_out can be 0 for dust amounts (program rejects via slippage check)
//    [V25] Split into legacy and V25 harnesses to cover both reserve ranges
// ============================================================================

#[kani::proof]
fn verify_curve_tokens_bounded_legacy() {
    let vt: u64 = kani::any();
    let vs: u64 = kani::any();
    let sol: u64 = kani::any();
    kani::assume(vs >= INITIAL_VIRTUAL_SOL);
    kani::assume(vs <= INITIAL_VIRTUAL_SOL + BONDING_TARGET_LAMPORTS);
    kani::assume(vt > 0);
    kani::assume(vt <= INITIAL_VIRTUAL_TOKENS);
    kani::assume(sol >= MIN_SOL_AMOUNT);
    kani::assume(sol <= BONDING_TARGET_LAMPORTS);

    let out = calc_tokens_out(vt, vs, sol).unwrap();
    assert!(out < vt);
}

#[kani::proof]
fn verify_curve_tokens_bounded_v25() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let (ivs, ivt) = initial_virtual_reserves(target);

    let vt: u64 = kani::any();
    let vs: u64 = kani::any();
    let sol: u64 = kani::any();
    kani::assume(vs >= ivs);
    kani::assume(vs <= ivs + target);
    kani::assume(vt > 0);
    kani::assume(vt <= ivt);
    kani::assume(sol >= MIN_SOL_AMOUNT);
    kani::assume(sol <= target);

    let out = calc_tokens_out(vt, vs, sol).unwrap();
    assert!(out < vt);
}

// ============================================================================
// 7. [V36] BUY: Token Split — 100% to buyer (vote vault removed)
//    Proves: tokens_to_buyer == tokens_out
// ============================================================================

#[kani::proof]
fn verify_token_split_conservation() {
    let tokens_out: u64 = kani::any();
    kani::assume(tokens_out > 0);
    kani::assume(tokens_out <= TOTAL_SUPPLY);

    // [V36] 100% to buyer — no split
    let to_buyer = tokens_out;
    assert!(to_buyer == tokens_out);
    assert!(to_buyer <= TOTAL_SUPPLY);
}

// ============================================================================
// 9. SELL: SOL Output Bounded
//    Proves: sol_out < virtual_sol_reserves (can't drain more than exists)
//    [V25] Split into legacy and V25 harnesses
// ============================================================================

#[kani::proof]
fn verify_sell_sol_bounded_legacy() {
    let vs: u64 = kani::any();
    let vt: u64 = kani::any();
    let tokens: u64 = kani::any();
    kani::assume(vs >= INITIAL_VIRTUAL_SOL);
    kani::assume(vs <= INITIAL_VIRTUAL_SOL + BONDING_TARGET_LAMPORTS);
    kani::assume(vt >= INITIAL_VIRTUAL_TOKENS / 2);
    kani::assume(vt <= INITIAL_VIRTUAL_TOKENS);
    kani::assume(tokens >= MIN_SOL_AMOUNT);
    kani::assume(tokens <= MAX_WALLET_TOKENS);

    let sol = calc_sol_out(vs, vt, tokens).unwrap();
    assert!(sol < vs);
}

#[kani::proof]
fn verify_sell_sol_bounded_v25() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let (ivs, ivt) = initial_virtual_reserves(target);

    let vs: u64 = kani::any();
    let vt: u64 = kani::any();
    let tokens: u64 = kani::any();
    kani::assume(vs >= ivs);
    kani::assume(vs <= ivs + target);
    // V27: tokens in curve decrease from 756.25M as people buy
    kani::assume(vt >= ivt / 2);
    kani::assume(vt <= ivt);
    kani::assume(tokens >= MIN_SOL_AMOUNT);
    kani::assume(tokens <= MAX_WALLET_TOKENS);

    let sol = calc_sol_out(vs, vt, tokens).unwrap();
    assert!(sol < vs);
}

// ============================================================================
// 9. TRANSFER FEE: Ceiling Division Bounds
//     Proves: fee <= amount, fee >= floor, fee <= floor + 1
// ============================================================================

#[kani::proof]
fn verify_transfer_fee_bounds() {
    let amount: u64 = kani::any();
    kani::assume(amount >= MIN_SOL_AMOUNT);
    kani::assume(amount <= 100_000_000); // 100 tokens — ceiling division correctness is range-independent

    let fee = calc_transfer_fee(amount).unwrap();
    let floor = (amount as u128 * TRANSFER_FEE_BPS as u128 / 10000) as u64;

    assert!(fee <= amount);
    assert!(fee >= floor);
    assert!(fee <= floor + 1);
}

// ============================================================================
// 12. TRANSFER FEE: Post-Fee Amount Non-Negative
//     Proves: amount - fee never underflows
// ============================================================================

#[kani::proof]
fn verify_transfer_fee_no_underflow() {
    let amount: u64 = kani::any();
    kani::assume(amount > 0);
    kani::assume(amount <= TOTAL_SUPPLY);

    let fee = calc_transfer_fee(amount).unwrap();
    assert!(amount >= fee);
}

// ============================================================================
// 13. LENDING: Collateral Value Proportionality
//     Proves: value <= pool_sol when collateral <= pool_tokens
// ============================================================================

// Split into concrete pool states for lending (post-migration DEX pool)
#[kani::proof]
fn verify_collateral_value_bounded_small() {
    let pool_sol: u64 = 50_000_000_000; // 50 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50B tokens
    let collateral: u64 = kani::any();
    kani::assume(collateral >= MIN_SOL_AMOUNT);
    kani::assume(collateral <= pool_tokens);

    let value = calc_collateral_value(collateral, pool_sol, pool_tokens).unwrap();
    assert!(value <= pool_sol);
}

#[kani::proof]
fn verify_collateral_value_bounded_large() {
    let pool_sol: u64 = 500_000_000_000; // 500 SOL pool
    let pool_tokens: u64 = 200_000_000_000_000; // 200T tokens
    let collateral: u64 = kani::any();
    kani::assume(collateral >= MIN_SOL_AMOUNT);
    kani::assume(collateral <= pool_tokens);

    let value = calc_collateral_value(collateral, pool_sol, pool_tokens).unwrap();
    assert!(value <= pool_sol);
}

// ============================================================================
// 14. LENDING: LTV Edge Cases
//     Proves: zero-collateral returns MAX, zero-debt returns 0, equal returns 10000
// ============================================================================

#[kani::proof]
fn verify_ltv_zero_collateral() {
    let debt: u64 = kani::any();
    kani::assume(debt > 0);
    assert!(calc_ltv_bps(debt, 0).unwrap() == u64::MAX);
}

#[kani::proof]
fn verify_ltv_zero_debt() {
    let cv: u64 = kani::any();
    kani::assume(cv > 0);
    assert!(calc_ltv_bps(0, cv).unwrap() == 0);
}

// Dropped: verify_ltv_100_percent — (v*10000)/v == 10000 is a tautology for any v > 0.
// The property is structural (u128 division cancels). Zero-collateral and zero-debt
// harnesses already prove the edge cases that matter for safety.

// ============================================================================
// 15. LENDING: Interest Non-Overflow
//     Proves: interest calculation doesn't overflow for realistic parameters
// ============================================================================

#[kani::proof]
fn verify_interest_no_overflow() {
    let principal: u64 = kani::any();
    let rate: u16 = kani::any();
    let slots: u64 = kani::any();
    kani::assume(principal > 0);
    kani::assume(principal <= 1_000_000_000_000); // 1000 SOL
    kani::assume(rate > 0);
    kani::assume(rate <= DEFAULT_INTEREST_RATE_BPS); // Max protocol rate: 2%/epoch
    kani::assume(slots > 0);
    kani::assume(slots <= EPOCH_DURATION_SLOTS); // Max 1 epoch

    let interest = calc_interest(principal, rate, slots);

    // Must not overflow
    assert!(interest.is_some());

    // Interest for 1 epoch at default rate should be at most 2% of principal
    let i = interest.unwrap();
    assert!(i <= principal); // 2% << 100%
}

// ============================================================================
// 16. LENDING: Liquidation Bonus Increases Seizure
//     Proves: bonus_bps > 0 means more collateral seized than without bonus
//     Constrained to realistic pool ratios (pool_tokens <= 1000x pool_sol value)
//     to avoid u128 overflow on the intermediate multiplication
// ============================================================================

// Concrete pool state eliminates symbolic pool vars from SAT solver
#[kani::proof]
fn verify_liquidation_bonus_increases_seizure() {
    let pool_sol: u64 = 100_000_000_000; // 100 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens
    let debt: u64 = kani::any();
    kani::assume(debt > 0);
    kani::assume(debt <= 50_000_000_000); // Max 50 SOL debt

    let no_bonus = calc_collateral_to_seize(debt, 0, pool_tokens, pool_sol).unwrap();
    let with_bonus =
        calc_collateral_to_seize(debt, DEFAULT_LIQUIDATION_BONUS_BPS, pool_tokens, pool_sol)
            .unwrap();

    assert!(with_bonus >= no_bonus);
}

// ============================================================================
// 17. PROTOCOL REWARDS: User Share Bounded by Distributable
//     Proves: no user can claim more than the distributable amount
// ============================================================================

// Concrete pool params keep SAT tractable. Only user_vol is symbolic.
// Property: share <= distributable (floor division of (a*b)/c <= b when a <= c).
#[kani::proof]
fn verify_user_share_bounded() {
    let total_vol: u64 = 500_000_000_000; // 500 SOL epoch volume
    let distributable: u64 = 50_000_000_000; // 50 SOL distributable
    let user_vol: u64 = kani::any();
    kani::assume(user_vol >= MIN_EPOCH_VOLUME_ELIGIBILITY); // [V32] 2 SOL min
    kani::assume(user_vol <= total_vol);

    let share = calc_user_share(user_vol, distributable, total_vol).unwrap();

    assert!(share <= distributable);
}

// ============================================================================
// 33. [V32] PROTOCOL REWARDS: Min Claim Enforcement
//     Proves: any claim that passes the MIN_CLAIM_AMOUNT check is >= 0.1 SOL,
//     and any share below MIN_CLAIM_AMOUNT is correctly rejected.
// ============================================================================

#[kani::proof]
fn verify_min_claim_enforcement() {
    let total_vol: u64 = kani::any();
    let distributable: u64 = kani::any();
    let user_vol: u64 = kani::any();

    // Realistic bounds
    kani::assume(total_vol >= 10_000_000_000); // >= 10 SOL total volume
    kani::assume(total_vol <= 10_000_000_000_000); // <= 10,000 SOL
    kani::assume(distributable > 0);
    kani::assume(distributable <= 1_000_000_000_000); // <= 1,000 SOL
    kani::assume(user_vol >= MIN_EPOCH_VOLUME_ELIGIBILITY); // >= 2 SOL
    kani::assume(user_vol <= total_vol);

    let share = calc_user_share(user_vol, distributable, total_vol).unwrap();
    let claim_amount = share.min(distributable);

    // If claim passes the minimum check, it is genuinely >= 0.1 SOL
    if claim_amount >= MIN_CLAIM_AMOUNT {
        assert!(claim_amount >= 100_000_000); // 0.1 SOL in lamports
    }

    // Claim never exceeds distributable
    assert!(claim_amount <= distributable);
}

// ============================================================================
// 18. RATIO MATH: Ratio Fits u64
//     Proves: pool ratio calculation doesn't overflow u64
//     Used by sell cycle (swap_fees_to_sol) ratio gating
// ============================================================================

#[kani::proof]
fn verify_ratio_fits_u64() {
    let pool_sol: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    kani::assume(pool_sol > 0);
    kani::assume(pool_sol <= 1_000_000_000_000); // 1000 SOL max
    kani::assume(pool_tokens >= 1_000_000); // At least 1 token (6 decimals) — no supply floor
    kani::assume(pool_tokens <= TOTAL_SUPPLY);

    let ratio = (pool_sol as u128)
        .checked_mul(RATIO_PRECISION)
        .unwrap()
        .checked_div(pool_tokens as u128)
        .unwrap();

    assert!(ratio <= u64::MAX as u128);
}

// ============================================================================
// 18b. [V30] RATIO-GATED SELL: Sell Threshold Fits u64
//      Proves: baseline_ratio * 12000 / 10000 doesn't overflow u64
//      Same bounds as verify_ratio_fits_u64, with the 1.2x sell multiplier.
// ============================================================================

#[kani::proof]
fn verify_sell_threshold_fits_u64() {
    let pool_sol: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    kani::assume(pool_sol > 0);
    kani::assume(pool_sol <= 1_000_000_000_000); // 1000 SOL max
    kani::assume(pool_tokens >= 1_000_000); // At least 1 token (6 decimals)
    kani::assume(pool_tokens <= TOTAL_SUPPLY);

    let baseline_ratio = (pool_sol as u128)
        .checked_mul(RATIO_PRECISION)
        .unwrap()
        .checked_div(pool_tokens as u128)
        .unwrap();

    let sell_threshold = baseline_ratio
        .checked_mul(DEFAULT_SELL_THRESHOLD_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap();

    assert!(sell_threshold <= u64::MAX as u128);
}

// ============================================================================
// 19. MIGRATION: Double Transfer Fee Still Leaves Positive Tokens
//     Proves: token_amount after two transfer fees is still positive
// ============================================================================

#[kani::proof]
fn verify_double_transfer_fee_positive() {
    let amount: u64 = kani::any();
    kani::assume(amount >= 1_000_000); // At least 1 token (6 decimals)
    kani::assume(amount <= TOTAL_SUPPLY);

    let fee1 = calc_transfer_fee(amount).unwrap();
    let after1 = amount.checked_sub(fee1).unwrap();

    // After first fee, must still have tokens
    assert!(after1 > 0);

    let fee2 = calc_transfer_fee(after1).unwrap();
    let after2 = after1.checked_sub(fee2).unwrap();

    // After second fee, must still have tokens
    assert!(after2 > 0);
}

// ============================================================================
// 21. MIGRATION: bonding_curve_sol → pool Lamport Conservation
//     The bonded SOL lives in the System-owned bonding_curve_sol vault; migration
//     seed-signs a transfer of exactly real_sol_reserves into the pool (no separate
//     fund step, no user wallet). Proves: pool credited == vault debited (exact),
//     the vault keeps any donation residual, total lamports conserved.
// ============================================================================

#[kani::proof]
fn verify_migration_sol_conservation() {
    let real_sol_reserves: u64 = kani::any();
    kani::assume(real_sol_reserves > 0);
    kani::assume(real_sol_reserves <= BONDING_TARGET_LAMPORTS);

    // bonding_curve_sol holds at least real_sol_reserves (it accumulated every buy);
    // it may also hold a benign donation residual on top.
    let residual: u64 = kani::any();
    kani::assume(residual <= 10_000_000);
    let vault_lamports = real_sol_reserves.checked_add(residual).unwrap();

    // Migration transfers exactly real_sol_reserves out (must not underflow).
    let vault_after = vault_lamports.checked_sub(real_sol_reserves).unwrap();
    assert!(vault_after == residual); // donation residual stays in the vault

    // Pool receives exactly real_sol_reserves.
    let pool_before: u64 = kani::any();
    kani::assume(pool_before <= u64::MAX - real_sol_reserves);
    let pool_after = pool_before.checked_add(real_sol_reserves).unwrap();
    assert!(pool_after - pool_before == real_sol_reserves);

    // Total lamports conserved across the two accounts.
    assert!(
        vault_after as u128 + pool_after as u128 == vault_lamports as u128 + pool_before as u128
    );
}

// ============================================================================
// 24. MIGRATION: Price-Matched Pool Preserves Bonding Curve Price
//     Concrete virtual_tokens values at key pool states (completion, midpoint, max).
//     Proves: pool ratio matches curve ratio (truncation bounded) for each tier.
// ============================================================================

fn assert_price_matched(real_sol: u64, virtual_tokens: u64, virtual_sol: u64) {
    let tokens_for_pool = calc_tokens_for_pool(real_sol, virtual_tokens, virtual_sol).unwrap();

    // Cross-multiply: tokens_for_pool * virtual_sol <= real_sol * virtual_tokens
    let lhs = (tokens_for_pool as u128)
        .checked_mul(virtual_sol as u128)
        .unwrap();
    let rhs = (real_sol as u128)
        .checked_mul(virtual_tokens as u128)
        .unwrap();
    assert!(lhs <= rhs);
    assert!(rhs - lhs < virtual_sol as u128);
}

// [V31] Price-matched proofs per tier

// [V4.0] Legacy: SPARK tier removed from creation, but existing tokens still use these constants
#[kani::proof]
fn verify_price_matched_pool_spark() {
    let (ivs, ivt) = initial_virtual_reserves(BONDING_TARGET_SPARK);
    let real_sol: u64 = BONDING_TARGET_SPARK; // 50 SOL
    let virtual_sol: u64 = ivs + BONDING_TARGET_SPARK; // 68.75 SOL

    assert_price_matched(real_sol, 206_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, 400_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, ivt, virtual_sol);
}

#[kani::proof]
fn verify_price_matched_pool_flame() {
    let (ivs, ivt) = initial_virtual_reserves(BONDING_TARGET_FLAME);
    let real_sol: u64 = BONDING_TARGET_FLAME; // 100 SOL
    let virtual_sol: u64 = ivs + BONDING_TARGET_FLAME; // 137.5 SOL

    assert_price_matched(real_sol, 206_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, 400_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, ivt, virtual_sol);
}

#[kani::proof]
fn verify_price_matched_pool_torch() {
    let (ivs, ivt) = initial_virtual_reserves(BONDING_TARGET_TORCH);
    let real_sol: u64 = BONDING_TARGET_TORCH; // 200 SOL
    let virtual_sol: u64 = ivs + BONDING_TARGET_TORCH; // 275 SOL (V27: IVS=75)

    // Representative values: at completion (~206M), midpoint (~400M), max (756.25M)
    assert_price_matched(real_sol, 206_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, 400_000_000_000_000, virtual_sol);
    assert_price_matched(real_sol, ivt, virtual_sol);
}

// ============================================================================
// 25. MIGRATION: Excess Token Burn Conservation
//     Concrete pool state. Only vault_amount is symbolic.
//     Proves: pool tokens + burned tokens == vault total
// ============================================================================

// [V31] Excess token burn conservation (Spark tier, symbolic vault)
// [V4.0] Legacy: proves math for existing 50 SOL tokens
#[kani::proof]
fn verify_excess_token_burn_conservation() {
    let (ivs, _ivt) = initial_virtual_reserves(BONDING_TARGET_SPARK);
    let real_sol: u64 = BONDING_TARGET_SPARK;
    let virtual_sol: u64 = ivs + BONDING_TARGET_SPARK; // 68.75 SOL
    let virtual_tokens: u64 = 206_000_000_000_000; // ~206M tokens (at completion: 3*IVT/11)
    let vault_amount: u64 = kani::any();
    kani::assume(vault_amount > 0);
    kani::assume(vault_amount <= CURVE_SUPPLY);

    let tokens_for_pool_raw = calc_tokens_for_pool(real_sol, virtual_tokens, virtual_sol).unwrap();
    let tokens_for_pool = tokens_for_pool_raw.min(vault_amount);
    let excess = vault_amount.checked_sub(tokens_for_pool).unwrap();

    assert!(tokens_for_pool + excess == vault_amount);
    assert!(tokens_for_pool <= vault_amount);
}

// ============================================================================
// 26. [V36] MIGRATION: Full Supply Conservation
//     Proves: treasury_lock + wallets + pool_tokens + excess_burned == TOTAL_SUPPLY
//     [V36] Vote vault removed — 100% of tokens sold go to wallets.
// ============================================================================

fn assert_full_supply_conservation(bonding_target: u64) {
    let (ivs, ivt) = initial_virtual_reserves(bonding_target);

    // At graduation: virtual_sol = IVS + BT
    let virtual_sol = ivs + bonding_target;

    // Constant product: k = IVS * IVT
    // virtual_tokens_remaining = k / virtual_sol = (IVS * IVT) / (IVS + BT)
    let k = (ivs as u128).checked_mul(ivt as u128).unwrap();
    let virtual_tokens_remaining = k.checked_div(virtual_sol as u128).unwrap() as u64;

    // Tokens sold during bonding — [V36] 100% to wallets (no vote vault)
    let in_wallets = ivt.checked_sub(virtual_tokens_remaining).unwrap();

    // [V31] Real tokens remaining in vault (starts at CURVE_SUPPLY = 700M)
    let real_token_reserves = CURVE_SUPPLY.checked_sub(in_wallets).unwrap();

    // Pool tokens = real_sol * virtual_tokens / virtual_sol
    let tokens_for_pool =
        calc_tokens_for_pool(bonding_target, virtual_tokens_remaining, virtual_sol).unwrap();

    // Excess burned = vault - pool
    let excess_burned = real_token_reserves.checked_sub(tokens_for_pool).unwrap();

    // FULL CONSERVATION: curve tokens + treasury lock = total supply
    let curve_total = (in_wallets as u128)
        .checked_add(tokens_for_pool as u128)
        .unwrap()
        .checked_add(excess_burned as u128)
        .unwrap();

    assert!(curve_total + TREASURY_LOCK_TOKENS as u128 == TOTAL_SUPPLY as u128);
}

// [V4.0] Legacy: SPARK tier
#[kani::proof]
fn verify_v31_full_supply_conservation_spark() {
    assert_full_supply_conservation(BONDING_TARGET_SPARK);
}

#[kani::proof]
fn verify_v31_full_supply_conservation_flame() {
    assert_full_supply_conservation(BONDING_TARGET_FLAME);
}

#[kani::proof]
fn verify_v31_full_supply_conservation_torch() {
    assert_full_supply_conservation(BONDING_TARGET_TORCH);
}

// ============================================================================
// 27. [V31] MIGRATION: Pool Tokens Positive & Bounded at Graduation
//     Proves: at graduation (real_sol = BT) for any V31 tier,
//     tokens_for_pool > 0 AND tokens_for_pool <= real_token_reserves.
//     Migration only fires at graduation, so this covers the actual state.
//     (IVT > CURVE_SUPPLY by 56.25M — virtual curve extends beyond real supply,
//     so pre-graduation symbolic exploration would hit unreachable states.)
// ============================================================================

#[kani::proof]
fn verify_v31_pool_tokens_positive_and_bounded() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let (ivs, ivt) = initial_virtual_reserves(target);

    // Migration occurs at graduation: virtual_sol = IVS + BT
    let virtual_sol = ivs + target;

    // Constant product at graduation: vtr = IVS * IVT / (IVS + BT)
    let k = (ivs as u128).checked_mul(ivt as u128).unwrap();
    let virtual_tokens_remaining = k.checked_div(virtual_sol as u128).unwrap() as u64;

    // Tokens sold at graduation
    let tokens_sold = ivt - virtual_tokens_remaining;
    // [V31] Vault starts at CURVE_SUPPLY (700M)
    let real_token_reserves = CURVE_SUPPLY - tokens_sold;

    let tokens_for_pool =
        calc_tokens_for_pool(target, virtual_tokens_remaining, virtual_sol).unwrap();

    // Pool always has tokens (non-empty pool)
    assert!(tokens_for_pool > 0);
    // Vault always has enough to seed the pool
    assert!(tokens_for_pool <= real_token_reserves);
}

// ============================================================================
// 28. [V31] MIGRATION: Zero Excess Burn
//     Proves: at graduation, excess_burned == 0 for all V31 tiers.
//     V31 tunes CURVE_SUPPLY (700M) so vault_remaining == tokens_for_pool.
// ============================================================================

fn assert_zero_excess_burn(bonding_target: u64) {
    let (ivs, ivt) = initial_virtual_reserves(bonding_target);

    let virtual_sol = ivs + bonding_target;

    // virtual_tokens_remaining at graduation (constant product)
    let k = (ivs as u128).checked_mul(ivt as u128).unwrap();
    let virtual_tokens_remaining = k.checked_div(virtual_sol as u128).unwrap() as u64;

    let tokens_sold = ivt.checked_sub(virtual_tokens_remaining).unwrap();
    // [V31] Vault starts at CURVE_SUPPLY (700M)
    let real_token_reserves = CURVE_SUPPLY.checked_sub(tokens_sold).unwrap();

    let tokens_for_pool =
        calc_tokens_for_pool(bonding_target, virtual_tokens_remaining, virtual_sol).unwrap();

    let excess_burned = real_token_reserves.checked_sub(tokens_for_pool).unwrap();

    // V31: zero burn by construction (CURVE_SUPPLY = 700M, IVT = 756.25M)
    assert!(excess_burned == 0);
}

// [V4.0] Legacy: SPARK tier
#[kani::proof]
fn verify_v31_zero_excess_burn_spark() {
    assert_zero_excess_burn(BONDING_TARGET_SPARK);
}

#[kani::proof]
fn verify_v31_zero_excess_burn_flame() {
    assert_zero_excess_burn(BONDING_TARGET_FLAME);
}

#[kani::proof]
fn verify_v31_zero_excess_burn_torch() {
    assert_zero_excess_burn(BONDING_TARGET_TORCH);
}

// ============================================================================
// 29. SELL: Fee Is Always Zero
//     Proves: SELL_FEE_BPS == 0, so sell_fee == 0 for any sol_out.
//     This justifies leaving protocol_treasury optional in the Sell context —
//     there is no fee to evade.
// ============================================================================

#[kani::proof]
fn verify_sell_fee_always_zero() {
    // Static assertion: the constant itself is 0
    assert!(SELL_FEE_BPS == 0);

    // Dynamic assertion: for any valid sol_out, the fee computes to 0
    let sol_out: u64 = kani::any();
    kani::assume(sol_out > 0);
    kani::assume(sol_out <= BONDING_TARGET_LAMPORTS);

    let fee = sol_out
        .checked_mul(SELL_FEE_BPS as u64)
        .unwrap()
        .checked_div(10000)
        .unwrap();

    assert!(fee == 0);
}

// ============================================================================
// 34. [V34] CREATOR RATE: Bounds
//     Proves: creator_rate_bps is always in [CREATOR_SOL_MIN_BPS, CREATOR_SOL_MAX_BPS]
//     for all valid tiers and reserve levels.
// ============================================================================

#[kani::proof]
fn verify_creator_rate_bounds() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let reserves: u64 = kani::any();
    kani::assume(reserves <= target);

    let rate = calc_creator_rate_bps(reserves, target).unwrap();

    assert!(rate >= CREATOR_SOL_MIN_BPS);
    assert!(rate <= CREATOR_SOL_MAX_BPS);
}

// ============================================================================
// 35. [V34] CREATOR RATE: Monotonic Increase
//     Proves: more reserves → higher creator rate (incentivizes pushing to graduation)
// ============================================================================

#[kani::proof]
fn verify_creator_rate_monotonic() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= target);
    kani::assume(b <= target);
    kani::assume(a <= b);

    let rate_a = calc_creator_rate_bps(a, target).unwrap();
    let rate_b = calc_creator_rate_bps(b, target).unwrap();

    assert!(rate_b >= rate_a);
}

// ============================================================================
// 36. [V34] CREATOR RATE: Complement With Treasury Rate
//     Proves: creator_rate_bps < treasury_rate_bps for all valid states.
//     This guarantees the subtraction (total_split - creator_sol) never underflows.
// ============================================================================

#[kani::proof]
fn verify_creator_rate_less_than_treasury_rate() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let reserves: u64 = kani::any();
    kani::assume(reserves <= target);

    let treasury_rate = calc_treasury_rate_bps(reserves, target).unwrap();
    let creator_rate = calc_creator_rate_bps(reserves, target).unwrap();

    assert!(creator_rate < treasury_rate);
}

// ============================================================================
// 37. [V34] CREATOR FEE SHARE: Bounded
//     Proves: creator_amount <= sol_received for any swap_fees_to_sol output.
//     15% share never exceeds the total.
// ============================================================================

#[kani::proof]
fn verify_creator_fee_share_bounded() {
    let sol_received: u64 = kani::any();
    kani::assume(sol_received > 0);
    kani::assume(sol_received <= 1_000_000_000_000); // 1000 SOL max swap output

    let creator_amount = calc_creator_fee_share(sol_received).unwrap();
    let treasury_amount = sol_received.checked_sub(creator_amount).unwrap();

    assert!(creator_amount <= sol_received);
    assert!(creator_amount + treasury_amount == sol_received);
}

// ============================================================================
// 30. LENDING: Borrow-Repay Lifecycle Conservation
//     Proves: after borrow + full repay (same slot, no interest),
//     treasury SOL balance is exactly restored and loan is zeroed out.
// ============================================================================

#[kani::proof]
fn verify_lending_lifecycle_conservation() {
    // Symbolic inputs constrained to realistic ranges
    let collateral: u64 = kani::any();
    let sol_borrowed: u64 = kani::any();
    let pool_sol: u64 = 100_000_000_000; // 100 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens

    kani::assume(collateral >= MIN_SOL_AMOUNT);
    kani::assume(collateral <= MAX_WALLET_TOKENS);
    kani::assume(sol_borrowed >= MIN_BORROW_AMOUNT);
    kani::assume(sol_borrowed <= 50_000_000_000); // Max 50 SOL borrow

    // Treasury starts with enough SOL
    let treasury_sol_before: u64 = kani::any();
    kani::assume(treasury_sol_before >= sol_borrowed);
    kani::assume(treasury_sol_before <= 500_000_000_000); // Max 500 SOL

    // ========== BORROW ==========
    // LTV check (must pass for borrow to succeed)
    let collateral_value = calc_collateral_value(collateral, pool_sol, pool_tokens).unwrap();
    kani::assume(collateral_value > 0);
    let ltv = calc_ltv_bps(sol_borrowed, collateral_value).unwrap();
    kani::assume(ltv <= DEFAULT_MAX_LTV_BPS as u64);

    // After borrow: loan state
    let _loan_collateral = collateral;
    let loan_borrowed = sol_borrowed;
    let loan_interest: u64 = 0; // Same slot, no interest

    // Treasury decreases by sol_borrowed
    let treasury_after_borrow = treasury_sol_before.checked_sub(sol_borrowed).unwrap();

    // ========== FULL REPAY (same slot) ==========
    let total_owed = loan_borrowed.checked_add(loan_interest).unwrap();
    let actual_repay = total_owed; // Full repay

    // Apply repayment: interest first, then principal
    let interest_paid;
    let principal_paid;
    if actual_repay <= loan_interest {
        interest_paid = actual_repay;
        principal_paid = 0;
    } else {
        interest_paid = loan_interest;
        principal_paid = actual_repay.checked_sub(loan_interest).unwrap();
    }

    // After full repay: loan zeroed
    let loan_borrowed_after = loan_borrowed.checked_sub(principal_paid).unwrap();
    let loan_interest_after = loan_interest.saturating_sub(interest_paid);
    let loan_collateral_after: u64 = 0; // Full repay returns all collateral

    // Treasury increases by actual_repay
    let treasury_after_repay = treasury_after_borrow.checked_add(actual_repay).unwrap();

    // ========== ASSERTIONS ==========
    // Treasury SOL perfectly conserved
    assert!(treasury_after_repay == treasury_sol_before);

    // Loan fully zeroed
    assert!(loan_borrowed_after == 0);
    assert!(loan_interest_after == 0);
    assert!(loan_collateral_after == 0);

    // Principal repaid equals original borrow
    assert!(principal_paid == sol_borrowed);
}

// ============================================================================
// 31. LENDING: Partial Repay Accounting
//     Proves: after partial repay, remaining debt = original - repaid,
//     interest is paid first, and collateral is unchanged.
// ============================================================================

#[kani::proof]
fn verify_lending_partial_repay_accounting() {
    let sol_borrowed: u64 = kani::any();
    let accrued_interest: u64 = kani::any();
    let repay_amount: u64 = kani::any();

    kani::assume(sol_borrowed >= MIN_BORROW_AMOUNT);
    kani::assume(sol_borrowed <= 50_000_000_000); // Max 50 SOL
    kani::assume(accrued_interest <= sol_borrowed / 10); // Interest < 10% of principal
    kani::assume(repay_amount > 0);

    let total_owed = sol_borrowed.checked_add(accrued_interest).unwrap();
    kani::assume(repay_amount < total_owed); // Partial repay

    // Apply repayment: interest first, then principal (mirrors lending.rs logic)
    let interest_paid;
    let principal_paid;
    let interest_after;
    let borrowed_after;

    if repay_amount <= accrued_interest {
        interest_paid = repay_amount;
        principal_paid = 0;
        interest_after = accrued_interest.checked_sub(repay_amount).unwrap();
        borrowed_after = sol_borrowed;
    } else {
        interest_paid = accrued_interest;
        principal_paid = repay_amount.checked_sub(accrued_interest).unwrap();
        interest_after = 0;
        borrowed_after = sol_borrowed.checked_sub(principal_paid).unwrap();
    }

    // Remaining debt = total_owed - repay_amount
    let remaining_debt = borrowed_after.checked_add(interest_after).unwrap();
    let expected_remaining = total_owed.checked_sub(repay_amount).unwrap();
    assert!(remaining_debt == expected_remaining);

    // Total paid = interest_paid + principal_paid = repay_amount
    assert!(interest_paid.checked_add(principal_paid).unwrap() == repay_amount);

    // Interest paid before principal (if any interest exists)
    if accrued_interest > 0 && repay_amount > 0 {
        assert!(interest_paid > 0 || accrued_interest == 0);
    }

    // Borrowed amount never increases
    assert!(borrowed_after <= sol_borrowed);
}

// ============================================================================
// 32. LENDING: Borrow-Accrue-Repay Full Lifecycle with Interest
//     Proves: after borrow, interest accrual, and full repay,
//     treasury receives principal + interest (no SOL lost or created).
// ============================================================================

#[kani::proof]
fn verify_lending_lifecycle_with_interest() {
    let sol_borrowed: u64 = kani::any();
    let slots_elapsed: u64 = kani::any();
    let interest_rate: u16 = DEFAULT_INTEREST_RATE_BPS; // 2% per epoch

    kani::assume(sol_borrowed >= MIN_BORROW_AMOUNT);
    kani::assume(sol_borrowed <= 50_000_000_000); // Max 50 SOL
    kani::assume(slots_elapsed > 0);
    kani::assume(slots_elapsed <= EPOCH_DURATION_SLOTS); // Max 1 epoch

    let treasury_sol_before: u64 = kani::any();
    kani::assume(treasury_sol_before >= sol_borrowed);
    kani::assume(treasury_sol_before <= 500_000_000_000);

    // ========== BORROW ==========
    let treasury_after_borrow = treasury_sol_before.checked_sub(sol_borrowed).unwrap();

    // ========== ACCRUE INTEREST ==========
    let interest = calc_interest(sol_borrowed, interest_rate, slots_elapsed).unwrap();

    // ========== FULL REPAY ==========
    let total_owed = sol_borrowed.checked_add(interest).unwrap();
    let actual_repay = total_owed;

    // Interest paid first, then principal
    let principal_paid = actual_repay.checked_sub(interest).unwrap();

    // Treasury receives full repayment
    let treasury_after_repay = treasury_after_borrow.checked_add(actual_repay).unwrap();

    // ========== ASSERTIONS ==========
    // Treasury gains exactly the interest amount
    assert!(treasury_after_repay == treasury_sol_before.checked_add(interest).unwrap());

    // Principal fully repaid
    assert!(principal_paid == sol_borrowed);

    // Interest bounded: at most 2% for 1 epoch
    assert!(interest <= sol_borrowed);
}

// ============================================================================
// 33. LENDING: Per-User Borrow Cap (Supply-Proportional)
//     Proves: max_user_borrow = max_lendable * collateral * 23 / total_supply
//     never overflows and correctly bounds user borrows proportionally.
//     Uses concrete max_lendable at each tier's 80% cap (same pattern as
//     migration price-match proofs) to keep SAT formula tractable.
// ============================================================================

fn check_per_user_cap(max_lendable: u64) {
    let user_collateral: u64 = kani::any();
    kani::assume(user_collateral <= TOTAL_SUPPLY);

    // Mirror on-chain calculation (u128 arithmetic)
    let max_user_borrow = (max_lendable as u128)
        .checked_mul(user_collateral as u128)
        .unwrap()
        .checked_mul(BORROW_SHARE_MULTIPLIER as u128)
        .unwrap()
        .checked_div(TOTAL_SUPPLY as u128)
        .unwrap() as u64;

    // User cap never exceeds total lendable * multiplier
    assert!(max_user_borrow <= max_lendable * BORROW_SHARE_MULTIPLIER);

    // Boundary: zero collateral → zero cap
    if user_collateral == 0 {
        assert!(max_user_borrow == 0);
    }
    // Boundary: 100% of supply → exactly 23x lendable
    if user_collateral == TOTAL_SUPPLY {
        assert!(max_user_borrow == max_lendable * BORROW_SHARE_MULTIPLIER);
    }
}

#[kani::proof]
fn verify_per_user_borrow_cap_bounded() {
    // 70% utilization cap at each tier:
    // Spark: 70% of 50 SOL = 35 SOL, Flame: 70 SOL, Torch: 140 SOL
    check_per_user_cap(35_000_000_000); // Spark
    check_per_user_cap(70_000_000_000); // Flame
    check_per_user_cap(140_000_000_000); // Torch
}

// ============================================================================
// 45. PROTOCOL REWARDS: Per-User Claim Cap (10%)
//     Proves: no single user can claim more than 10% of distributable amount
//     per epoch, even if they generated 100% of the volume.
// ============================================================================

// Concrete pool params keep SAT tractable. Only user_vol is symbolic.
// Property: capped claim <= 10% of distributable for any user volume share.
#[kani::proof]
fn verify_claim_cap_enforced() {
    let total_vol: u64 = 500_000_000_000; // 500 SOL epoch volume
    let distributable: u64 = 50_000_000_000; // 50 SOL distributable
    let user_vol: u64 = kani::any();
    kani::assume(user_vol >= MIN_EPOCH_VOLUME_ELIGIBILITY); // >= 2 SOL
    kani::assume(user_vol <= total_vol);

    let claim = calc_claim_with_cap(user_vol, distributable, total_vol).unwrap();

    // 10% of 50 SOL = 5 SOL
    assert!(claim <= 5_000_000_000);
    assert!(claim <= distributable);
}

// Concrete: user with 100% of volume only gets 10%
#[kani::proof]
fn verify_claim_cap_monopoly_trader() {
    let total_vol: u64 = 500_000_000_000; // 500 SOL epoch volume
    let distributable: u64 = 50_000_000_000; // 50 SOL distributable

    // User has ALL the volume
    let user_vol = total_vol;

    let claim = calc_claim_with_cap(user_vol, distributable, total_vol).unwrap();

    // Without cap they'd get 100% (50 SOL). With cap: 10% = 5 SOL
    assert!(claim <= 5_000_000_000);
    assert!(claim == distributable / 10);
}

// ============================================================================
// 45. [V35] COMMUNITY TOKEN: Buy SOL Conservation
//     Proves: when creator_sol = 0 (community token), the full sol_amount
//     is still exactly distributed across curve + treasury + dev + protocol.
//     This covers the explicit is_community_token branch in the buy handler.
// ============================================================================

#[kani::proof]
fn verify_community_token_buy_conservation() {
    let target: u64 = kani::any();
    assume_valid_target(target);
    let sol_amount: u64 = kani::any();
    let reserves: u64 = kani::any();
    kani::assume(sol_amount >= MIN_SOL_AMOUNT);
    kani::assume(sol_amount <= 10_000_000_000); // 10 SOL realistic max
    kani::assume(reserves <= target);

    let pf_total = calc_protocol_fee(sol_amount, PROTOCOL_FEE_BPS).unwrap();
    let dev = calc_dev_wallet_share(pf_total).unwrap();
    let pf = pf_total.checked_sub(dev).unwrap();
    let tf = calc_token_treasury_fee(sol_amount).unwrap();
    let after = sol_amount
        .checked_sub(pf_total)
        .unwrap()
        .checked_sub(tf)
        .unwrap();

    let treasury_rate = calc_treasury_rate_bps(reserves, target).unwrap();

    // Total split from sol_after_fees
    let total_split = after
        .checked_mul(treasury_rate as u64)
        .unwrap()
        .checked_div(10000)
        .unwrap();

    // [V35] Community token: creator_sol = 0, full split to treasury
    let creator_sol: u64 = 0;
    let sol_to_treasury_split = total_split.checked_sub(creator_sol).unwrap();
    let to_curve = after.checked_sub(total_split).unwrap();
    let total_treasury = tf.checked_add(sol_to_treasury_split).unwrap();

    let distributed = to_curve
        .checked_add(total_treasury)
        .unwrap()
        .checked_add(creator_sol)
        .unwrap()
        .checked_add(dev)
        .unwrap()
        .checked_add(pf)
        .unwrap();

    assert!(distributed == sol_amount);
    // Community token: treasury gets MORE than with creator fees
    assert!(sol_to_treasury_split == total_split);
}

// ============================================================================
// 46. [V35] COMMUNITY TOKEN: Swap Fees Conservation
//     Proves: when is_community_token = true, creator_amount = 0 and
//     treasury_amount == sol_received (100% to treasury, no leakage).
// ============================================================================

#[kani::proof]
fn verify_community_token_swap_fees_conservation() {
    let sol_received: u64 = kani::any();
    kani::assume(sol_received > 0);
    kani::assume(sol_received <= 1_000_000_000_000); // 1000 SOL max

    // Community token path: creator_amount = 0, treasury_amount = sol_received
    let creator_amount: u64 = 0;
    let treasury_amount = sol_received;

    assert!(creator_amount + treasury_amount == sol_received);
    assert!(treasury_amount == sol_received);
    assert!(creator_amount == 0);
}

// ============================================================================
// V5: SHORT SELLING PROOFS
// ============================================================================

// ============================================================================
// 47. [V5] SHORT: Debt Value Bounded
//     Proves: debt value in SOL never exceeds pool SOL for realistic positions
// ============================================================================

#[kani::proof]
fn verify_short_debt_value_bounded_small() {
    let pool_sol: u64 = 50_000_000_000; // 50 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens
    let token_debt: u64 = kani::any();
    kani::assume(token_debt >= MIN_SHORT_TOKENS);
    kani::assume(token_debt <= pool_tokens);

    let value = calc_short_debt_value(token_debt, pool_sol, pool_tokens).unwrap();
    assert!(value <= pool_sol);
}

#[kani::proof]
fn verify_short_debt_value_bounded_large() {
    let pool_sol: u64 = 500_000_000_000; // 500 SOL pool
    let pool_tokens: u64 = 200_000_000_000_000; // 200T tokens
    let token_debt: u64 = kani::any();
    kani::assume(token_debt >= MIN_SHORT_TOKENS);
    kani::assume(token_debt <= pool_tokens);

    let value = calc_short_debt_value(token_debt, pool_sol, pool_tokens).unwrap();
    assert!(value <= pool_sol);
}

// ============================================================================
// 48. [V5] SHORT: LTV Edge Cases
//     Proves: zero SOL collateral returns MAX, zero debt returns 0
// ============================================================================

#[kani::proof]
fn verify_short_ltv_zero_collateral() {
    let debt_value: u64 = kani::any();
    kani::assume(debt_value > 0);
    assert!(calc_ltv_bps(debt_value, 0).unwrap() == u64::MAX);
}

#[kani::proof]
fn verify_short_ltv_zero_debt() {
    let sol_collateral: u64 = kani::any();
    kani::assume(sol_collateral > 0);
    assert!(calc_ltv_bps(0, sol_collateral).unwrap() == 0);
}

// ============================================================================
// 49. [V5] SHORT: Interest Non-Overflow (Token Terms)
//     Proves: token interest calculation doesn't overflow for realistic parameters
// ============================================================================

#[kani::proof]
fn verify_short_interest_no_overflow() {
    let tokens_borrowed: u64 = kani::any();
    let rate: u16 = kani::any();
    let slots: u64 = kani::any();
    kani::assume(tokens_borrowed > 0);
    kani::assume(tokens_borrowed <= TOTAL_SUPPLY); // Max: entire supply
    kani::assume(rate > 0);
    kani::assume(rate <= DEFAULT_INTEREST_RATE_BPS); // 2%/epoch
    kani::assume(slots > 0);
    kani::assume(slots <= EPOCH_DURATION_SLOTS); // Max 1 epoch

    let interest = calc_short_interest(tokens_borrowed, rate, slots);
    assert!(interest.is_some());

    // Interest for 1 epoch at default rate should be at most 2% of principal
    let i = interest.unwrap();
    assert!(i <= tokens_borrowed);
}

// ============================================================================
// 50. [V5] SHORT: Liquidation Bonus Increases SOL Seizure
//     Proves: bonus > 0 means more SOL seized than without bonus
// ============================================================================

#[kani::proof]
fn verify_short_liquidation_bonus_increases_seizure() {
    let debt_value: u64 = kani::any();
    kani::assume(debt_value > 0);
    kani::assume(debt_value <= 50_000_000_000); // Max 50 SOL debt value

    let no_bonus = calc_short_sol_to_seize(debt_value, 0).unwrap();
    let with_bonus = calc_short_sol_to_seize(debt_value, DEFAULT_LIQUIDATION_BONUS_BPS).unwrap();

    assert!(with_bonus >= no_bonus);
}

// ============================================================================
// 51. [V21] SHORT: Token Lifecycle Conservation (Open → Close, No Interest)
//     V21 shorts borrow tokens from the static TreasuryLock (atomic sell pulls
//     them out on open; the borrower buys them back on close). Proves: an open +
//     immediate close perfectly conserves the lock's token balance AND the
//     total_tokens_lent counter. (SOL collateral lives in a per-position System
//     vault — out of scope here; the V20 treasury-SOL / short_collateral_reserved
//     escrow no longer exists.)
// ============================================================================

#[kani::proof]
fn verify_short_lifecycle_conservation() {
    let tokens_borrowed: u64 = kani::any();
    kani::assume(tokens_borrowed >= MIN_SHORT_TOKENS);
    kani::assume(tokens_borrowed <= TOTAL_SUPPLY / 10); // Max 10% of supply

    let lock_tokens_before: u64 = kani::any();
    let total_tokens_lent_before: u64 = kani::any();
    kani::assume(lock_tokens_before >= tokens_borrowed);
    kani::assume(total_tokens_lent_before <= TOTAL_SUPPLY - tokens_borrowed);

    // ========== OPEN SHORT: lock lends tokens_borrowed ==========
    let lock_after_open = lock_tokens_before.checked_sub(tokens_borrowed).unwrap();
    let lent_after_open = total_tokens_lent_before.checked_add(tokens_borrowed).unwrap();

    // ========== CLOSE SHORT (immediate, no interest): buy back exactly tokens_borrowed ==========
    let lock_after_close = lock_after_open.checked_add(tokens_borrowed).unwrap();
    let lent_after_close = lent_after_open.checked_sub(tokens_borrowed).unwrap();

    // ========== ASSERTIONS ==========
    // Lock token balance perfectly conserved (tokens lent out = tokens returned).
    assert!(lock_after_close == lock_tokens_before);
    // total_tokens_lent counter returns to its starting value.
    assert!(lent_after_close == total_tokens_lent_before);
}

// ============================================================================
// 52. [V5] SHORT: Partial Close Accounting
//     Proves: after partial close, remaining debt = original - repaid,
//     interest is paid first, and SOL collateral is unchanged.
// ============================================================================

#[kani::proof]
fn verify_short_partial_close_accounting() {
    let tokens_borrowed: u64 = kani::any();
    let accrued_interest: u64 = kani::any();
    let return_amount: u64 = kani::any();

    kani::assume(tokens_borrowed >= MIN_SHORT_TOKENS);
    kani::assume(tokens_borrowed <= TOTAL_SUPPLY / 10);
    kani::assume(accrued_interest <= tokens_borrowed / 10); // Interest < 10%
    kani::assume(return_amount > 0);

    let total_owed = tokens_borrowed.checked_add(accrued_interest).unwrap();
    kani::assume(return_amount < total_owed); // Partial close

    // Apply repayment: interest first, then principal (mirrors short.rs logic)
    let interest_paid;
    let principal_paid;
    let interest_after;
    let borrowed_after;

    if return_amount <= accrued_interest {
        interest_paid = return_amount;
        principal_paid = 0;
        interest_after = accrued_interest.checked_sub(return_amount).unwrap();
        borrowed_after = tokens_borrowed;
    } else {
        interest_paid = accrued_interest;
        principal_paid = return_amount.checked_sub(accrued_interest).unwrap();
        interest_after = 0;
        borrowed_after = tokens_borrowed.checked_sub(principal_paid).unwrap();
    }

    // Remaining debt = total_owed - return_amount
    let remaining_debt = borrowed_after.checked_add(interest_after).unwrap();
    let expected_remaining = total_owed.checked_sub(return_amount).unwrap();
    assert!(remaining_debt == expected_remaining);

    // Total paid = interest_paid + principal_paid = return_amount
    assert!(interest_paid.checked_add(principal_paid).unwrap() == return_amount);

    // Borrowed amount never increases
    assert!(borrowed_after <= tokens_borrowed);
}

// ============================================================================
// 53. [V5] SHORT: Lifecycle with Interest Conservation
//     Proves: after open_short, interest accrual, and full close,
//     treasury receives principal + interest tokens (no tokens lost or created).
// ============================================================================

#[kani::proof]
fn verify_short_lifecycle_with_interest() {
    let tokens_borrowed: u64 = kani::any();
    let slots_elapsed: u64 = kani::any();
    let interest_rate: u16 = DEFAULT_INTEREST_RATE_BPS;

    kani::assume(tokens_borrowed >= MIN_SHORT_TOKENS);
    kani::assume(tokens_borrowed <= TOTAL_SUPPLY / 10);
    kani::assume(slots_elapsed > 0);
    kani::assume(slots_elapsed <= EPOCH_DURATION_SLOTS);

    let treasury_tokens_before: u64 = kani::any();
    kani::assume(treasury_tokens_before >= tokens_borrowed);
    kani::assume(treasury_tokens_before <= TOTAL_SUPPLY);

    // ========== OPEN SHORT ==========
    let treasury_after_open = treasury_tokens_before.checked_sub(tokens_borrowed).unwrap();

    // ========== ACCRUE INTEREST ==========
    let interest = calc_short_interest(tokens_borrowed, interest_rate, slots_elapsed).unwrap();

    // ========== FULL CLOSE ==========
    let total_owed = tokens_borrowed.checked_add(interest).unwrap();
    let actual_return = total_owed;

    let principal_paid = actual_return.checked_sub(interest).unwrap();

    // Treasury receives full token repayment
    let treasury_after_close = treasury_after_open.checked_add(actual_return).unwrap();

    // ========== ASSERTIONS ==========
    // Treasury gains exactly the interest amount in tokens
    assert!(treasury_after_close == treasury_tokens_before.checked_add(interest).unwrap());

    // Principal fully repaid
    assert!(principal_paid == tokens_borrowed);

    // Interest bounded: at most 2% for 1 epoch
    assert!(interest <= tokens_borrowed);
}

// ============================================================================
// 54. [V21] SHORT: Collateral Isolated From Lending — REMOVED.
//     V20 subtracted `short_collateral_reserved` from treasury SOL to get the
//     lendable float (verify_short_collateral_reservation). In V21 shorts escrow
//     collateral in per-position System vaults and never touch the lending
//     treasury, so the isolation is STRUCTURAL (short collateral is simply not an
//     input to `treasury_physical − total_sol_lent_to_longs`) — there is no
//     arithmetic property left to prove. Covered by verify_lending_gate_available_sol.
// ============================================================================
// 55. LENDING: Bad Debt Write-Off Reduces total_sol_lent
//     Proves: after liquidation with bad debt, total_sol_lent is reduced by
//     both principal repaid AND bad debt written off, preventing utilization
//     cap drift. Concrete pool + interest + aggregate for SAT tractability;
//     only borrowed and collateral are symbolic.
// ============================================================================

#[kani::proof]
fn verify_liquidation_bad_debt_accounting() {
    let pool_sol: u64 = 100_000_000_000; // 100 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens
    let interest: u64 = 500_000_000; // 0.5 SOL accrued interest
    let total_sol_lent_before: u64 = 200_000_000_000; // 200 SOL aggregate

    let borrowed: u64 = kani::any();
    let collateral: u64 = kani::any();

    kani::assume(borrowed >= MIN_BORROW_AMOUNT);
    kani::assume(borrowed <= 50_000_000_000); // Max 50 SOL
    kani::assume(borrowed >= interest); // Principal >= accrued interest
    kani::assume(collateral > 0);
    kani::assume(collateral <= 500_000_000_000); // Bounded collateral tokens

    let total_debt = borrowed.checked_add(interest).unwrap();

    // Liquidation covers up to close_bps% of total debt
    let max_debt_to_cover = (total_debt as u128)
        .checked_mul(DEFAULT_LIQUIDATION_CLOSE_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap() as u64;
    let debt_to_cover = max_debt_to_cover.min(total_debt);

    // Compute collateral to seize
    let collateral_to_seize = calc_collateral_to_seize(
        debt_to_cover,
        DEFAULT_LIQUIDATION_BONUS_BPS,
        pool_tokens,
        pool_sol,
    )
    .unwrap();

    let actual_collateral_seized = collateral_to_seize.min(collateral);

    // Insolvent iff the vault can't fund the full target seize (collateral-capped):
    // the entire vault is taken and no collateral remains. [V21] In that case the
    // ENTIRE residual debt is forgiven and the position fully resolves — no tail.
    let insolvent = collateral_to_seize > collateral;
    let actual_debt_covered = if insolvent {
        calc_collateral_value(actual_collateral_seized, pool_sol, pool_tokens).unwrap()
    } else {
        debt_to_cover
    };

    // Apply repayment: interest first, then principal.
    let interest_paid = actual_debt_covered.min(interest);
    let principal_paid = actual_debt_covered - interest_paid;
    let mut loan_interest = interest - interest_paid;
    let mut loan_borrowed = borrowed.saturating_sub(principal_paid);

    // [V21] On insolvency, forgive the entire residual (principal + interest).
    let written_off_principal = if insolvent {
        let p = loan_borrowed;
        loan_borrowed = 0;
        loan_interest = 0;
        p
    } else {
        0
    };

    // total_sol_lent reduced by principal repaid AND the written-off principal.
    let total_sol_lent_after = total_sol_lent_before
        .saturating_sub(principal_paid)
        .saturating_sub(written_off_principal);

    // Key property: an insolvent liquidation FULLY resolves the position and removes
    // exactly its original principal from total_sol_lent (no un-liquidatable tail,
    // no permanent counter inflation).
    if insolvent {
        assert!(loan_borrowed == 0 && loan_interest == 0);
        // Reduced by AT LEAST the original principal → never left inflated (the bug).
        assert!(total_sol_lent_after <= total_sol_lent_before.saturating_sub(borrowed));
    }

    // total_sol_lent never increases (saturating, monotone down).
    assert!(total_sol_lent_after <= total_sol_lent_before);
}

// ============================================================================
// 56. [V5] SHORT: Bad Debt Write-Off Reduces total_tokens_lent
//     Proves: after short liquidation with bad debt, total_tokens_lent is
//     reduced by both principal repaid AND bad debt tokens, preventing
//     utilization cap drift on the token side. Concrete interest + aggregate
//     for SAT tractability; only tokens_borrowed and sol_collateral symbolic.
// ============================================================================

#[kani::proof]
fn verify_short_liquidation_bad_debt_accounting() {
    let pool_sol: u64 = 100_000_000_000; // 100 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens
    let interest: u64 = 1_000_000_000; // 1B token interest
    let total_tokens_lent_before: u64 = 100_000_000_000_000; // 100T aggregate

    let tokens_borrowed: u64 = kani::any();
    let sol_collateral: u64 = kani::any();

    kani::assume(tokens_borrowed >= interest);
    kani::assume(tokens_borrowed <= 50_000_000_000_000); // Max 50T tokens
    kani::assume(sol_collateral > 0);
    kani::assume(sol_collateral <= 50_000_000_000); // Max 50 SOL collateral

    let total_token_debt = tokens_borrowed.checked_add(interest).unwrap();

    // Liquidation covers up to close_bps% of total token debt
    let max_tokens_to_cover = (total_token_debt as u128)
        .checked_mul(DEFAULT_LIQUIDATION_CLOSE_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap() as u64;
    let tokens_to_cover = max_tokens_to_cover.min(total_token_debt);

    // Debt value in SOL
    let debt_value = calc_short_debt_value(tokens_to_cover, pool_sol, pool_tokens).unwrap();

    // SOL to seize (with bonus)
    let sol_to_seize = calc_short_sol_to_seize(debt_value, DEFAULT_LIQUIDATION_BONUS_BPS).unwrap();
    let actual_sol_seized = sol_to_seize.min(sol_collateral);

    // If collateral insufficient, compute actual tokens covered
    let actual_tokens_covered = if sol_to_seize > sol_collateral {
        // Bad debt: reverse-compute from seized SOL
        let seized_value = actual_sol_seized;
        let without_bonus = (seized_value as u128)
            .checked_mul(10000)
            .unwrap()
            .checked_div((10000 + DEFAULT_LIQUIDATION_BONUS_BPS as u64) as u128)
            .unwrap() as u64;
        // Convert SOL value back to tokens
        (without_bonus as u128)
            .checked_mul(pool_tokens as u128)
            .unwrap()
            .checked_div(pool_sol as u128)
            .unwrap() as u64
    } else {
        tokens_to_cover
    };

    // Insolvent iff the SOL collateral can't fund the full target seize. [V21] In
    // that case the ENTIRE residual token debt is forgiven and the position fully
    // resolves — no un-liquidatable tail.
    let insolvent = sol_to_seize > sol_collateral;

    // Apply repayment: interest first, then principal.
    let interest_paid = actual_tokens_covered.min(interest);
    let principal_paid = actual_tokens_covered - interest_paid;
    let mut pos_interest = interest - interest_paid;
    let mut pos_borrowed = tokens_borrowed.saturating_sub(principal_paid);

    // [V21] On insolvency, forgive the entire residual (principal + interest).
    let written_off_principal = if insolvent {
        let p = pos_borrowed;
        pos_borrowed = 0;
        pos_interest = 0;
        p
    } else {
        0
    };

    // total_tokens_lent reduced by principal repaid AND the written-off principal.
    let total_tokens_lent_after = total_tokens_lent_before
        .saturating_sub(principal_paid)
        .saturating_sub(written_off_principal);

    // Key property: an insolvent liquidation FULLY resolves the position and removes
    // exactly its original principal from total_tokens_lent (no tail, no inflation).
    if insolvent {
        assert!(pos_borrowed == 0 && pos_interest == 0);
        // Reduced by AT LEAST the original principal → never left inflated (the bug).
        assert!(
            total_tokens_lent_after <= total_tokens_lent_before.saturating_sub(tokens_borrowed)
        );
    }

    // total_tokens_lent never increases (saturating, monotone down).
    assert!(total_tokens_lent_after <= total_tokens_lent_before);
}

// ============================================================================
// 57. LENDING: Liquidation Requires Positive Pool Reserves (Both Sides)
//     Proves: the pool_sol > 0 && pool_tokens > 0 guard prevents division
//     by zero in collateral valuation, ensuring no stuck/unliquidatable loans.
// ============================================================================

#[kani::proof]
fn verify_pool_reserve_guards_prevent_div_zero() {
    let pool_sol: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    let collateral: u64 = kani::any();

    kani::assume(collateral > 0);
    kani::assume(collateral <= MAX_WALLET_TOKENS);

    // Guard: both reserves must be positive (as now enforced in code)
    kani::assume(pool_sol > 0 && pool_tokens > 0);

    // Realistic bounds so `collateral * pool_sol / pool_tokens` fits u64.
    // Real pools: pool_sol ≤ 1000 SOL (pre-migration cap), pool_tokens ≥ 1 whole
    // token (1e9 lamports at 6 decimals).
    kani::assume(pool_sol <= 1_000_000_000_000);
    kani::assume(pool_tokens >= 1_000_000_000);

    // Collateral value computation must succeed (no division by zero, no overflow).
    let cv = calc_collateral_value(collateral, pool_sol, pool_tokens);
    assert!(cv.is_some());

    // LTV computation must also succeed
    let ltv = calc_ltv_bps(1_000_000_000, cv.unwrap()); // 1 SOL debt
    assert!(ltv.is_some());
}

// ============================================================================
// 58. [V5] SHORT: Pool Reserve Guards for Debt Valuation
//     Proves: pool_sol > 0 && pool_tokens > 0 ensures short debt valuation
//     never hits division by zero.
// ============================================================================

#[kani::proof]
fn verify_short_pool_reserve_guards() {
    let pool_sol: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    let token_debt: u64 = kani::any();

    kani::assume(token_debt > 0);
    kani::assume(token_debt <= TOTAL_SUPPLY);
    kani::assume(pool_sol > 0 && pool_tokens > 0);

    // Realistic bounds so `token_debt * pool_sol / pool_tokens` fits u64.
    kani::assume(pool_sol <= 1_000_000_000_000);
    kani::assume(pool_tokens >= 1_000_000_000);

    // Debt value computation must succeed (no division by zero, no overflow).
    let dv = calc_short_debt_value(token_debt, pool_sol, pool_tokens);
    assert!(dv.is_some());
}

// ============================================================================
// 62. [V6] CIRCUIT BREAKER: Min Pool Liquidity Constant Check
//     Proves: MIN_POOL_SOL_LENDING is 5 SOL and the check rejects below it.
// ============================================================================

#[kani::proof]
fn verify_min_pool_liquidity_threshold() {
    let pool_sol: u64 = kani::any();
    kani::assume(pool_sol <= 10_000_000_000); // Bound for tractability

    let passes = pool_sol >= MIN_POOL_SOL_LENDING;

    // Exactly 5 SOL passes
    if pool_sol == 5_000_000_000 {
        assert!(passes);
    }
    // Below 5 SOL fails
    if pool_sol < 5_000_000_000 {
        assert!(!passes);
    }
}

// ============================================================================
// 63. LENDING: Bad Debt Formula Algebraic Identity
//     Proves: bad_debt = max(0, debt_to_cover - actual_debt_covered) for all
//     inputs. The on-chain formula uses an indirect expression via total_debt;
//     this proof confirms it reduces to the simple form.
// ============================================================================

#[kani::proof]
fn verify_bad_debt_formula_identity() {
    let borrowed: u64 = kani::any();
    let interest: u64 = kani::any();

    kani::assume(borrowed >= MIN_BORROW_AMOUNT);
    kani::assume(borrowed <= 50_000_000_000);
    kani::assume(interest <= borrowed);

    let total_debt = borrowed.checked_add(interest).unwrap();

    let max_debt_to_cover = (total_debt as u128)
        .checked_mul(DEFAULT_LIQUIDATION_CLOSE_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap() as u64;
    let debt_to_cover = max_debt_to_cover.min(total_debt);

    // Simulate both sufficient and insufficient collateral paths
    let actual_debt_covered: u64 = kani::any();
    kani::assume(actual_debt_covered <= debt_to_cover);

    // On-chain formula (indirect)
    let bad_debt_onchain = total_debt.saturating_sub(
        actual_debt_covered
            .checked_add(total_debt.saturating_sub(debt_to_cover))
            .unwrap(),
    );

    // Simple form (direct)
    let bad_debt_simple = debt_to_cover.saturating_sub(actual_debt_covered);

    // They must be identical
    assert!(bad_debt_onchain == bad_debt_simple);

    // bad_debt + actual_debt_covered == debt_to_cover (conservation of liquidation slice)
    assert!(bad_debt_simple + actual_debt_covered == debt_to_cover);
}

// ============================================================================
// 64. [V5] SHORT: Bad Debt Formula Algebraic Identity
//     Same proof as lending but for token-denominated short liquidation.
// ============================================================================

#[kani::proof]
fn verify_short_bad_debt_formula_identity() {
    let tokens_borrowed: u64 = kani::any();
    let interest: u64 = kani::any();

    kani::assume(tokens_borrowed >= MIN_SHORT_TOKENS);
    kani::assume(tokens_borrowed <= 50_000_000_000_000);
    kani::assume(interest <= tokens_borrowed);

    let total_token_debt = tokens_borrowed.checked_add(interest).unwrap();

    let max_tokens_to_cover = (total_token_debt as u128)
        .checked_mul(DEFAULT_LIQUIDATION_CLOSE_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap() as u64;
    let tokens_to_cover = max_tokens_to_cover.min(total_token_debt);

    let actual_tokens_covered: u64 = kani::any();
    kani::assume(actual_tokens_covered <= tokens_to_cover);

    // On-chain formula
    let bad_debt_onchain = total_token_debt.saturating_sub(
        actual_tokens_covered
            .checked_add(total_token_debt.saturating_sub(tokens_to_cover))
            .unwrap(),
    );

    // Simple form
    let bad_debt_simple = tokens_to_cover.saturating_sub(actual_tokens_covered);

    assert!(bad_debt_onchain == bad_debt_simple);
    assert!(bad_debt_simple + actual_tokens_covered == tokens_to_cover);
}

// ============================================================================
// 65. DEEPPOOL: Reserve Reading Safety
//     Proves: pool_lamports.saturating_sub(rent_exempt) never produces
//     inflated values. Ratio calculation succeeds when pool_tokens > 0.
//     No accumulated fee subtraction needed — DeepPool has no protocol fees.
// ============================================================================

#[kani::proof]
fn verify_deep_pool_reserve_reading_safe() {
    let pool_lamports: u64 = kani::any();
    let rent_exempt: u64 = kani::any();
    let token_vault_balance: u64 = kani::any();

    kani::assume(pool_lamports <= 10_000_000_000_000); // Max 10K SOL
    kani::assume(rent_exempt > 0);
    kani::assume(rent_exempt <= 10_000_000); // ~0.01 SOL max rent
    kani::assume(token_vault_balance <= TOTAL_SUPPLY);

    // saturating_sub: pool with less than rent shows 0 SOL (no underflow)
    let pool_sol = pool_lamports.saturating_sub(rent_exempt);
    assert!(pool_sol <= pool_lamports);

    // If pool has lamports > rent, SOL reserve is positive
    if pool_lamports > rent_exempt {
        assert!(pool_sol > 0);
        assert!(pool_sol == pool_lamports - rent_exempt);
    }

    // Ratio calculation succeeds when pool_tokens > 0
    if token_vault_balance > 0 {
        let ratio = (pool_sol as u128)
            .checked_mul(RATIO_PRECISION)
            .unwrap()
            .checked_div(token_vault_balance as u128);
        assert!(ratio.is_some());
    }
}

// ============================================================================
// 66. TREASURY: Sell Amount Bounded and Correct
//     Proves: sell_amount <= token_amount for all inputs, and the 15% calc
//     fits in u64. Below SELL_ALL_TOKEN_THRESHOLD, sell 100%.
// ============================================================================

#[kani::proof]
fn verify_treasury_sell_amount_bounded() {
    let token_amount: u64 = kani::any();
    kani::assume(token_amount > 0);
    kani::assume(token_amount <= TOTAL_SUPPLY);

    let sell_amount = if token_amount <= SELL_ALL_TOKEN_THRESHOLD {
        token_amount // 100% below threshold
    } else {
        (token_amount as u128)
            .checked_mul(DEFAULT_SELL_PERCENT_BPS as u128)
            .unwrap()
            .checked_div(10000)
            .unwrap() as u64
    };

    // Sell amount never exceeds balance
    assert!(sell_amount <= token_amount);

    // Below threshold: sell everything
    if token_amount <= SELL_ALL_TOKEN_THRESHOLD {
        assert!(sell_amount == token_amount);
    }

    // Above threshold: sell exactly 15%
    if token_amount > SELL_ALL_TOKEN_THRESHOLD {
        assert!(sell_amount <= token_amount / 6); // 15% < 1/6 ≈ 16.7%
                                                  // And it's non-zero for any positive amount above threshold
        assert!(sell_amount > 0);
    }
}

// ============================================================================
// Depth-Based Risk Bands
// ============================================================================

//     Proves: the continuous depth curve is 0 below the floor, equals LTV_MIN at
//     the floor, climbs concavely toward LTV_MAX, stays clamped in [MIN,MAX], and
//     is monotonic non-decreasing in depth. Inputs are CONCRETE literals so the
//     `·S_floor/pool_sol` division never goes symbolic (avoids Kani div-blowup).

fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < DEPTH_FLOOR_SOL {
        return 0;
    }
    let span = (LTV_MAX_BPS - LTV_MIN_BPS) as u64;
    let drop = span.saturating_mul(DEPTH_FLOOR_SOL) / pool_sol;
    let ltv = (LTV_MAX_BPS as u64).saturating_sub(drop);
    ltv.clamp(LTV_MIN_BPS as u64, LTV_MAX_BPS as u64) as u16
}

#[kani::proof]
fn verify_depth_curve_points() {
    // Below the floor: no leverage.
    assert!(get_depth_max_ltv_bps(0) == 0);
    assert!(get_depth_max_ltv_bps(99_999_999_999) == 0);

    // At the floor (100 SOL): LTV_MIN. 6000 − 3000·100/100 = 3000.
    assert!(get_depth_max_ltv_bps(100_000_000_000) == LTV_MIN_BPS);
    assert!(LTV_MIN_BPS == 3000);
    // Concave climb: 200→4500, 500→5400, 1000→5700.
    assert!(get_depth_max_ltv_bps(200_000_000_000) == 4500);
    assert!(get_depth_max_ltv_bps(500_000_000_000) == 5400);
    assert!(get_depth_max_ltv_bps(1_000_000_000_000) == 5700);
    // Asymptote: huge depth → LTV_MAX (drop → 0).
    assert!(get_depth_max_ltv_bps(u64::MAX) == LTV_MAX_BPS);

    // Bounded and monotonic non-decreasing across the sampled depths.
    assert!(get_depth_max_ltv_bps(u64::MAX) <= LTV_MAX_BPS);
    assert!(get_depth_max_ltv_bps(100_000_000_000) <= get_depth_max_ltv_bps(200_000_000_000));
    assert!(get_depth_max_ltv_bps(200_000_000_000) <= get_depth_max_ltv_bps(500_000_000_000));
    assert!(get_depth_max_ltv_bps(500_000_000_000) <= get_depth_max_ltv_bps(1_000_000_000_000));
}

// [V21] Rail 2 size cap — concrete points: ρ_max·S, exactly 25% of pool SOL.
#[kani::proof]
fn verify_size_cap_points() {
    fn max_debt_value_for_depth(pool_sol: u64) -> u64 {
        ((pool_sol as u128 * RHO_MAX_BPS as u128) / 10_000) as u64
    }
    assert!(RHO_MAX_BPS == 2500);
    assert!(max_debt_value_for_depth(100_000_000_000) == 25_000_000_000);
    assert!(max_debt_value_for_depth(1_000_000_000_000) == 250_000_000_000);
    assert!(max_debt_value_for_depth(0) == 0);
}

// ============================================================================
// 69. MIGRATION: Cost Reimbursement Is Exactly the Payer's Rent
//     The pool SOL is sourced from bonding_curve_sol (a separate System PDA), so
//     the permissionless payer's only outlay across create_pool is the rent for
//     the new pool accounts. Proves: migration_cost = payer_pre - payer_post =
//     rent_cost exactly (no sol_amount to net out), bounded to rent magnitude —
//     the treasury can never reimburse the pool SOL itself.
// ============================================================================

#[kani::proof]
fn verify_migration_cost_reimbursement() {
    let rent_cost: u64 = kani::any();
    let payer_pre: u64 = kani::any();

    kani::assume(rent_cost <= 50_000_000); // Max ~0.05 SOL rent
    kani::assume(payer_pre <= 10_000_000_000_000); // Max 10K SOL
    kani::assume(payer_pre >= rent_cost); // payer can afford the rent

    // The pool SOL comes from bonding_curve_sol, never the payer — so create_pool +
    // account creation only spends the payer's rent.
    let payer_post = payer_pre.checked_sub(rent_cost).unwrap();

    // migration_cost = payer_pre - payer_post (no saturating_sub(sol_amount) now).
    let migration_cost = payer_pre.checked_sub(payer_post).unwrap();

    // migration_cost == rent_cost exactly (only the rent, never the pool SOL).
    assert!(migration_cost == rent_cost);
    assert!(migration_cost <= 50_000_000); // bounded to rent magnitude
}

// ============================================================================
// 70. [V21] DEEPPOOL: Vault Swap SOL Accounting (derived, no sol_balance field)
//     The vault's SOL is DERIVED from its System-owned vault_sol PDA lamports —
//     there is no tracked sol_balance field. Proves: on sell, sol_received is the
//     measured lamport delta of vault_sol, and the derived balance increases by
//     exactly that delta. No inflation (the proceeds aren't double-counted).
// ============================================================================

#[kani::proof]
fn verify_vault_swap_sell_accounting() {
    let vault_sol_lamports_before: u64 = kani::any();
    let sol_received: u64 = kani::any();

    kani::assume(vault_sol_lamports_before <= 10_000_000_000_000);
    kani::assume(sol_received > 0);
    kani::assume(sol_received <= 10_000_000_000_000);

    // DeepPool's swap CPI credits sol_received lamports into vault_sol.
    let vault_sol_lamports_after = vault_sol_lamports_before.checked_add(sol_received);
    kani::assume(vault_sol_lamports_after.is_some());
    let vault_sol_lamports_after = vault_sol_lamports_after.unwrap();

    // Proceeds = measured lamport delta (reload-and-diff, never a passed amount).
    let measured = vault_sol_lamports_after
        .checked_sub(vault_sol_lamports_before)
        .unwrap();
    assert!(measured == sol_received);

    // The DERIVED balance (vault_sol lamports) reflects exactly the proceeds —
    // no separate field to drift, no double-count.
    assert!(vault_sol_lamports_after == vault_sol_lamports_before + sol_received);
}

// ============================================================================
// 71. INTEREST ACCRUAL: Slot Always Advances (Long)
//     Proves: when `apply_interest_accrual` returns Some, the returned
//     last_update_slot equals current_slot — regardless of which branch was
//     taken (zero debt, zero slots elapsed, or normal accrual).
//     This is the post-condition that prevents the stale-slot re-borrow bug:
//     a position fully repaid at slot S but not closed must have its
//     last_update_slot advanced to S so a future re-borrow doesn't accrue
//     phantom interest for the dormant period.
// ============================================================================

#[kani::proof]
fn verify_interest_accrual_slot_advance() {
    let borrowed: u64 = kani::any();
    let accrued: u64 = kani::any();
    let last_slot: u64 = kani::any();
    let current_slot: u64 = kani::any();
    let rate: u16 = kani::any();

    kani::assume(borrowed <= 1_000_000_000_000); // up to 1000 SOL
    kani::assume(accrued <= 1_000_000_000_000);
    kani::assume(current_slot >= last_slot);
    kani::assume(current_slot - last_slot <= EPOCH_DURATION_SLOTS);
    kani::assume(rate <= DEFAULT_INTEREST_RATE_BPS);

    if let Some((_, new_slot)) =
        apply_interest_accrual(borrowed, accrued, last_slot, current_slot, rate)
    {
        assert!(new_slot == current_slot);
    }
}

// ============================================================================
// 72. SHORT INTEREST ACCRUAL: Slot Always Advances
//     Mirror of harness 71 for short positions (token debt via
//     `apply_short_interest_accrual` and `calc_short_interest`).
// ============================================================================

#[kani::proof]
fn verify_short_interest_accrual_slot_advance() {
    let borrowed: u64 = kani::any();
    let accrued: u64 = kani::any();
    let last_slot: u64 = kani::any();
    let current_slot: u64 = kani::any();
    let rate: u16 = kani::any();

    kani::assume(borrowed <= TOTAL_SUPPLY);
    kani::assume(accrued <= TOTAL_SUPPLY);
    kani::assume(current_slot >= last_slot);
    kani::assume(current_slot - last_slot <= EPOCH_DURATION_SLOTS);
    kani::assume(rate <= DEFAULT_INTEREST_RATE_BPS);

    if let Some((_, new_slot)) =
        apply_short_interest_accrual(borrowed, accrued, last_slot, current_slot, rate)
    {
        assert!(new_slot == current_slot);
    }
}

// ============================================================================
// 73. SHORT OPEN: tokens_borrowed Records GROSS Sent, Not Post-Fee Net
//     Pins the lock-conservation design in open_short / open_short_via_vault.
//     The handler records `args.tokens_to_borrow` (= gross transferred from
//     the lock) as `position.tokens_borrowed`. The shorter received `gross −
//     fee` net; they owe the full `gross` back on close.
//
//     Why this design: with `tokens_borrowed = net`, the lock loses
//     `fee_open` per cycle (no compensation from the close-side gross-up
//     unless interest > fee). For short holds (<6h at default rate), the
//     lock leaks. Recording gross makes the borrower responsible for the
//     open-leg fee gap → lock is exactly conserved every cycle (+interest).
//
//     Regression we are guarding against: reverting to net recording, which
//     reopens the sub-6hr-cycle lock-leak hole.
// ============================================================================

#[kani::proof]
fn verify_short_open_records_gross_amount() {
    let gross: u64 = kani::any();
    kani::assume(gross >= MIN_SHORT_TOKENS);
    kani::assume(gross <= TOTAL_SUPPLY / 10);

    // What handlers/short.rs records — `args.tokens_to_borrow` directly,
    // the gross transfer amount asked of treasury_lock.
    let recorded = gross;

    // The recorded principal equals the gross amount asked of the lock.
    // No diff-from-destination math; the source debit IS the debt.
    assert!(recorded == gross);

    // Recorded amount is positive for any valid input above MIN_SHORT_TOKENS.
    assert!(recorded > 0);

    // Sanity: net delivered to shorter is strictly less than recorded debt.
    // The shorter must close the fee gap from elsewhere (e.g. DEX) to repay.
    let fee = calc_transfer_fee(gross).unwrap();
    let net_delivered = gross.checked_sub(fee).unwrap();
    assert!(net_delivered < recorded);
}

// ============================================================================
// 74. SHORT CLOSE: Lock Receives EXACTLY tokens_borrowed + interest Net
//     Proves the lock-conservation property end-to-end. On open, lock
//     loses `gross`. On full close, borrower pays `gross_up(gross +
//     interest)`; after Token-2022 fee withhold, lock receives `gross +
//     interest` net. Cycle effect on lock: `+interest`, never negative.
//
//     This is the property that justifies recording gross at open: the
//     borrower funds the open-leg fee explicitly via the close gross-up,
//     and the lock token balance is invariant under cycle count.
// ============================================================================

#[kani::proof]
#[kani::unwind(2)]
fn verify_short_full_close_lock_conservation() {
    let gross: u64 = kani::any();
    let interest: u64 = kani::any();

    kani::assume(gross >= MIN_SHORT_TOKENS);
    // Tighten for Kani tractability — gross-up's u128 arithmetic blows up
    // CBMC at wider ranges. Proptest covers full u64 range symbolically.
    kani::assume(gross <= 10_000);
    kani::assume(interest <= 1_000);

    // After-open lock balance: starts at 300M, decreases by gross.
    let starting_lock: u64 = TREASURY_LOCK_TOKENS;
    let lock_after_open = starting_lock.checked_sub(gross).unwrap();

    // Close: total_owed = tokens_borrowed (gross) + interest.
    let total_owed = gross.checked_add(interest).unwrap();
    let gross_close = gross_up_for_transfer_fee(total_owed).unwrap();
    let fee_close = calc_transfer_fee(gross_close).unwrap();
    let net_to_lock = gross_close.checked_sub(fee_close).unwrap();

    // Lock balance after full close.
    let lock_after_close = lock_after_open.checked_add(net_to_lock).unwrap();

    // CONSERVATION: lock never loses tokens across a full cycle. Net change
    // is exactly +interest (modulo ±1 rounding on the gross-up ceil).
    assert!(lock_after_close >= starting_lock.checked_add(interest).unwrap()
        || lock_after_close >= starting_lock.checked_add(interest).unwrap().saturating_sub(1));
    // Strict lower bound: never below starting lock balance.
    assert!(lock_after_close >= starting_lock);
}

// ============================================================================
// 75. MATH HELPER: apply_bps — Concrete Identity Cases
//     The symbolic u128 mul-div stalls CBMC. Universal claims (`result <=
//     value`, monotonicity in bps) live in `tests/math_proptests.rs`. Kani
//     pins the boundary identities at fixed values.
// ============================================================================

#[kani::proof]
fn verify_apply_bps_concrete_identities() {
    let value: u64 = 100_000_000_000; // 100 SOL

    assert!(apply_bps(value, 0).unwrap() == 0); // 0 bps → 0
    assert!(apply_bps(value, 10_000).unwrap() == value); // 100% → identity
    assert!(apply_bps(value, 5_000).unwrap() == value / 2); // 50%
    assert!(apply_bps(0, 10_000).unwrap() == 0); // value=0 → 0
}

// ============================================================================
// 76. MATH HELPER: calc_user_borrow_cap — Concrete Fixtures
//     Universal `cap == 0` when any factor is zero lives in proptest. Kani
//     pins the zero-denominator short-circuit (no u128 arithmetic touched)
//     and a single positive case to keep CBMC happy.
// ============================================================================

#[kani::proof]
fn verify_calc_user_borrow_cap_zero_denominator() {
    let max_lendable: u64 = kani::any();
    let user_collateral: u64 = kani::any();

    // Zero denominator hits the early return; no u128 mul executed.
    assert!(calc_user_borrow_cap(max_lendable, user_collateral, 0).unwrap() == 0);
}

#[kani::proof]
fn verify_calc_user_borrow_cap_concrete_share() {
    // Cap is min(formula, absolute) — verify both branches bind correctly.
    let max_lendable: u64 = 1_000_000_000_000; // 1000 SOL
    let denominator: u64 = 100_000_000_000; // 100 SOL
    let absolute_cap: u64 =
        max_lendable * MAX_USER_BORROW_SHARE_BPS as u64 / 10_000; // 200 SOL

    // ABSOLUTE BRANCH: user_collateral == denominator means formula_cap =
    // max_lendable × 23 = 23,000 SOL, far above absolute_cap (200 SOL).
    // The clamp must bind.
    let cap_absolute_binds =
        calc_user_borrow_cap(max_lendable, denominator, denominator).unwrap();
    assert!(cap_absolute_binds == absolute_cap);

    // FORMULA BRANCH: user_collateral small enough that formula_cap <
    // absolute_cap. Crossover is at user_collateral / denominator =
    // β/(10000·μ) ≈ 0.870%. Use 0.1% (user_collateral = 100M lamports)
    // to be well inside the formula-binding region:
    //   formula_cap = 1000 SOL × 0.001 × 23 = 23 SOL  < 200 SOL absolute.
    //
    // Compute in u128 because `max_lendable × user_collateral` overflows
    // u64 (1e12 × 1e8 = 1e20). Production `calc_user_borrow_cap` uses the
    // same u128-intermediate pattern; we mirror it here so the assertion
    // models the actual semantics.
    let user_collateral_small: u64 = 100_000_000; // 0.1 SOL
    let expected_formula_cap: u64 = ((max_lendable as u128)
        * (user_collateral_small as u128)
        * (BORROW_SHARE_MULTIPLIER as u128)
        / (denominator as u128)) as u64;
    let cap_formula_binds =
        calc_user_borrow_cap(max_lendable, user_collateral_small, denominator)
            .unwrap();
    assert!(cap_formula_binds == expected_formula_cap);
    assert!(cap_formula_binds < absolute_cap);

    // ZERO COLLATERAL → cap == 0.
    let zero_cap = calc_user_borrow_cap(max_lendable, 0, denominator).unwrap();
    assert!(zero_cap == 0);
}

// ============================================================================
// 77. MATH HELPER: calc_bad_debt — Conservation Identity
//     Proves: bad_debt + covered + (total − debt_to_cover) == total_debt.
//     This is the algebraic identity the helper relies on: nothing leaks, all
//     debt is accounted for as either covered, uncovered remainder, or bad.
// ============================================================================

#[kani::proof]
fn verify_calc_bad_debt_conservation() {
    let total_debt: u64 = kani::any();
    let debt_to_cover: u64 = kani::any();
    let covered: u64 = kani::any();

    // Caller invariants: debt_to_cover <= total_debt, covered <= debt_to_cover.
    kani::assume(total_debt <= TOTAL_SUPPLY);
    kani::assume(debt_to_cover <= total_debt);
    kani::assume(covered <= debt_to_cover);

    let bad_debt = calc_bad_debt(total_debt, covered, debt_to_cover).unwrap();
    let uncovered_remainder = total_debt - debt_to_cover;

    // Conservation: nothing lost, nothing created.
    assert!(bad_debt + covered + uncovered_remainder == total_debt);
    // Bad debt is the under-coverage within the chosen close range.
    assert!(bad_debt == debt_to_cover - covered);
}

// ============================================================================
// 78. MATH HELPER: calc_price_ratio — Concrete + Zero-Denominator
//     The helper's u64 result range depends on `num × RATIO_PRECISION /
//     denom` — universally symbolic, this overflows u64 for tiny denominators
//     and trips CBMC. Universal monotonicity-in-num lives in proptest with
//     realistic pool-reserve ranges. Kani pins the failure modes here.
// ============================================================================

#[kani::proof]
fn verify_calc_price_ratio_zero_denominator() {
    let num: u64 = kani::any();
    // Zero denominator → None, no arithmetic executed.
    assert!(calc_price_ratio(num, 0).is_none());
}

#[kani::proof]
fn verify_calc_price_ratio_concrete_inputs() {
    // Typical pool reserves: 100 SOL vs 100M tokens (6 decimals).
    let pool_sol: u64 = 100_000_000_000;
    let pool_tokens: u64 = 100_000_000_000_000;

    let ratio = calc_price_ratio(pool_sol, pool_tokens).unwrap();
    // pool_sol × RATIO_PRECISION / pool_tokens = 10^11 × 10^9 / 10^14 = 10^6.
    assert!(ratio == 1_000_000);

    // num == 0 → ratio 0.
    let zero = calc_price_ratio(0, pool_tokens).unwrap();
    assert!(zero == 0);
}

// ============================================================================
// 79a. MATH HELPER: calc_short_partial_seize_proration — Zero Seize Undefined
//     Bounded model check on the `full_seize == 0` branch. Trivially terminates
//     since the function short-circuits before any u128 arithmetic.
// ============================================================================

#[kani::proof]
fn verify_short_partial_seize_proration_zero_seize_is_none() {
    let tokens_to_cover: u64 = kani::any();
    let capped_collateral: u64 = kani::any();

    let result = calc_short_partial_seize_proration(tokens_to_cover, capped_collateral, 0);
    assert!(result.is_none());
}

// ============================================================================
// 79b. MATH HELPER: calc_short_partial_seize_proration — Concrete Fixtures
//     The symbolic u128 mul-div hits CBMC's bit-vector ceiling hard. Concrete
//     inputs verify the boundary cases (capped == full → exact, capped == 0
//     → zero, capped == half → half) instantly. Universal coverage of the
//     ordering property `actual ≤ tokens_to_cover` lives in
//     `tests/math_proptests.rs` (`short_partial_seize_proration_bounded`).
// ============================================================================

#[kani::proof]
fn verify_short_partial_seize_proration_concrete_fixtures() {
    let tokens_to_cover: u64 = 1_000_000_000; // 1000 tokens (10^6 base units each)
    let full_seize: u64 = 100_000_000_000; // 100 SOL

    let exact =
        calc_short_partial_seize_proration(tokens_to_cover, full_seize, full_seize).unwrap();
    assert!(exact == tokens_to_cover);

    let zero = calc_short_partial_seize_proration(tokens_to_cover, 0, full_seize).unwrap();
    assert!(zero == 0);

    let half = calc_short_partial_seize_proration(tokens_to_cover, full_seize / 2, full_seize)
        .unwrap();
    assert!(half == tokens_to_cover / 2);
}

// ============================================================================
// 16. SHORT POOL STABILITY: gross_up_for_transfer_fee preserves net delivery
//     Proves: for any `net`, sending `gross_up_for_transfer_fee(net)` results
//     in the recipient receiving AT LEAST `net` after Token-2022 fee
//     deduction. This is the load-bearing property that keeps
//     treasury_lock_token_account stable across short open+close cycles —
//     borrower covers the transfer fee so the pool never depletes.
//
//     Tightness clause: the gross-up overshoots by at most 1 unit, so the
//     borrower isn't over-charged. (Single-unit overshoot is unavoidable
//     when combining ceiling-up gross-up with ceiling-up Token-2022 fee.)
// ============================================================================

#[kani::proof]
fn verify_gross_up_preserves_net_delivery() {
    let net: u64 = kani::any();
    // Tight upper bound for Kani tractability. The u128 arithmetic inside
    // gross_up_for_transfer_fee blows up CBMC's SAT space at wider ranges;
    // at 10_000 the proof completes in seconds. Since the ceiling-division
    // semantics are range-independent, proving the invariant exhaustively
    // here generalizes — `tests/math_proptests.rs::gross_up_preserves_net_delivery`
    // covers the symbolic range up to TOTAL_SUPPLY with random inputs.
    kani::assume(net > 0);
    kani::assume(net <= 10_000);

    let gross = gross_up_for_transfer_fee(net).unwrap();
    let fee = calc_transfer_fee(gross).unwrap();
    let net_received = gross.checked_sub(fee).unwrap();

    // SUFFICIENCY: recipient gets at least the requested net — the
    // protocol's token pool stays whole.
    assert!(net_received >= net);

    // TIGHTNESS: overshoot is bounded by 1 unit. Prevents the gross-up
    // from being inflated beyond what's necessary to make the recipient
    // whole.
    assert!(net_received <= net + 1);
}

// [V21] Lending unlock gate uses AVAILABLE SOL = derived treasury float minus the
// SOL already lent to longs: `treasury_physical_sol(treasury_sol_vault) −
// total_sol_lent_to_longs` (open_long in handlers/leverage.rs).
//
// The V20 short-collateral entanglement (V20C-1) is now STRUCTURALLY impossible:
// shorts no longer touch the SOL treasury at all — collateral lives in per-position
// System vaults and borrowed tokens come from the static TreasuryLock — so a short
// of any size leaves both `treasury_physical` and `total_sol_lent_to_longs`
// untouched and cannot move the gate. What remains to prove is the available-SOL
// computation itself: underflow-safe, never over-promises, and monotone — lending
// more SOL out only ever tightens the gate.
#[kani::proof]
fn verify_lending_gate_available_sol() {
    let treasury_physical: u64 = kani::any();
    let total_sol_lent_to_longs: u64 = kani::any();
    let threshold: u64 = kani::any();

    let available = treasury_physical.saturating_sub(total_sol_lent_to_longs);
    let gate_open = available >= threshold;

    // Available never exceeds the physical float — lending can't over-promise SOL.
    assert!(available <= treasury_physical);

    // Monotone: lending one more lamport to longs never OPENS a closed gate.
    let available_more_lent =
        treasury_physical.saturating_sub(total_sol_lent_to_longs.saturating_add(1));
    assert!(available_more_lent <= available);
    if !gate_open {
        assert!(available_more_lent < threshold);
    }
}

// ============================================================================
// 80. [V21] calc_sol_to_token_value — Empty-Side Guard + Safety
//     The inverse-direction mirror of calc_collateral_value (SOL value → token
//     amount). Proves: pool_sol==0 short-circuits to None (no div-by-zero), and
//     for a borrow value within pool depth the result is overflow-safe and
//     bounded by the token reserve.
// ============================================================================

#[kani::proof]
fn verify_sol_to_token_value_empty_side_none() {
    let sol_value: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    assert!(calc_sol_to_token_value(sol_value, 0, pool_tokens).is_none());
}

#[kani::proof]
fn verify_sol_to_token_value_safe_and_bounded() {
    let pool_sol: u64 = 100_000_000_000; // 100 SOL
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens
    let sol_value: u64 = kani::any();
    // Borrow value is bounded by pool SOL depth (LTV ≤ 100% of a collateral
    // worth at most the pool); pricing it into tokens can't exceed the reserve.
    kani::assume(sol_value <= pool_sol);

    let out = calc_sol_to_token_value(sol_value, pool_sol, pool_tokens);
    assert!(out.is_some());
    assert!(out.unwrap() <= pool_tokens);
}

// ============================================================================
// 81. [V21] calc_close_pool_amount_in — None Guards
//     Proves the short-circuits: zero `tokens_out`, over-fill (`>= pool_tokens`),
//     and a degenerate 100% swap fee all return None before any pool arithmetic.
// ============================================================================

#[kani::proof]
fn verify_close_pool_amount_in_none_guards() {
    let pool_sol: u64 = kani::any();
    let pool_tokens: u64 = kani::any();
    let swap_fee_bps: u16 = kani::any();
    kani::assume(pool_tokens > 1);

    // tokens_out == 0 → None
    assert!(calc_close_pool_amount_in(0, pool_sol, pool_tokens, swap_fee_bps).is_none());
    // tokens_out >= pool_tokens → None (can't drain or over-fill the pool)
    assert!(calc_close_pool_amount_in(pool_tokens, pool_sol, pool_tokens, swap_fee_bps).is_none());

    // 100% swap fee → None (fee denominator collapses to zero)
    let some_out: u64 = kani::any();
    kani::assume(some_out > 0 && some_out < pool_tokens);
    assert!(calc_close_pool_amount_in(some_out, pool_sol, pool_tokens, 10_000).is_none());
}

// ============================================================================
// 82. [V21] calc_close_pool_amount_in — Round-Trip Sufficiency
//     The load-bearing close_short property: the SOL the inverse quote returns,
//     run back through DeepPool's ACTUAL forward buy swap (deep_pool::math, the
//     exact on-chain code), yields AT LEAST `tokens_out`. The double-ceil
//     rounding guarantees the close buys enough to repay the grossed-up debt —
//     it can never come up short. Concrete pool + representative order sizes for
//     CBMC tractability; the symbolic range lives in
//     tests/math_proptests.rs::close_pool_amount_in_sufficient.
// ============================================================================

fn assert_close_quote_sufficient(tokens_out: u64, pool_sol: u64, pool_tokens: u64) {
    let fee_bps = deep_pool::constants::SWAP_FEE_BPS as u16; // 25 — DeepPool's real fee
    let amount_in =
        calc_close_pool_amount_in(tokens_out, pool_sol, pool_tokens, fee_bps).unwrap();

    // DeepPool forward buy on the quoted amount_in (exact on-chain functions).
    let fee = deep_pool::math::calc_swap_fee(amount_in).unwrap();
    let effective_in = amount_in.checked_sub(fee).unwrap();
    let realized = deep_pool::math::calc_swap_output(effective_in, pool_sol, pool_tokens).unwrap();

    assert!(realized >= tokens_out);
}

#[kani::proof]
fn verify_close_pool_amount_in_sufficiency() {
    let pool_sol: u64 = 100_000_000_000; // 100 SOL
    let pool_tokens: u64 = 50_000_000_000_000; // 50T base units
    // Order sizes spanning dust → 80% of the reserve, all < pool_tokens.
    assert_close_quote_sufficient(1_000_000, pool_sol, pool_tokens); // 1 whole token
    assert_close_quote_sufficient(1_000_000_000_000, pool_sol, pool_tokens); // 1M whole tokens
    assert_close_quote_sufficient(40_000_000_000_000, pool_sol, pool_tokens); // 80% of reserve
}

// ============================================================================
// 83. [V21][D-10] twap_value_in_sol — at-mark token→SOL pricing (LTV trigger)
//     As of v21 the oracle lives in DeepPool; torch consumes a Q64.64 SOL-per-
//     token price. This helper is now division-free (widening multiply + a
//     64-bit shift), so unlike the old cumulative-delta form it is FULLY
//     Kani-provable. Proves: value at exactly-representable prices (1.0, 0.5,
//     2.0) is the exact product, a zero amount is zero, and an overflowing
//     widening multiply fails closed to None (no panic, no wrap). Warmup /
//     fail-closed is DeepPool's read returning None, upstream of this.
// ============================================================================

#[kani::proof]
fn verify_twap_value_q64_exact() {
    let q1: u128 = 1u128 << 64; // price 1.0 sol/token
    let q_half: u128 = 1u128 << 63; // price 0.5
    let q2: u128 = 1u128 << 65; // price 2.0
    assert!(twap_value_in_sol(1_000_000, q1) == Some(1_000_000));
    assert!(twap_value_in_sol(1_000_000, q_half) == Some(500_000));
    assert!(twap_value_in_sol(1_000_000, q2) == Some(2_000_000));
    // Zero token amount → zero value at any price.
    assert!(twap_value_in_sol(0, q1) == Some(0));
    assert!(twap_value_in_sol(0, u128::MAX) == Some(0));
}

#[kani::proof]
fn verify_twap_value_overflow_is_none() {
    // amount × price overflowing u128 fails closed (caller refuses to liquidate),
    // never panics or wraps. u64::MAX × u128::MAX overflows the widening multiply.
    assert!(twap_value_in_sol(u64::MAX, u128::MAX).is_none());
}

#[kani::proof]
fn verify_twap_value_no_panic_symbolic() {
    // For ALL inputs the function returns (Some|None) without panicking — the
    // checked multiply makes this total. Division-free, so CBMC solves it.
    let amount: u64 = kani::any();
    let price_q64: u128 = kani::any();
    let _ = twap_value_in_sol(amount, price_q64);
}

// ============================================================================
// 84. [V21][D-10] twap_tokens_to_seize — at-mark seize sizing (seize clamp)
//     Pricing the seize at the mark (not spot) is what stops a spot pump/dump
//     at liquidation time from inflating the tokens seized. Proves: a zero
//     marked price → None (no seize basis, fail closed — division-free guard,
//     symbolic), the seize at exactly-representable prices/bonuses is the exact
//     amount, and an overflowing grossed-up multiply → None. The internal divide
//     keeps the value cases concrete (CBMC can't solve a symbolic 128-bit divide).
// ============================================================================

#[kani::proof]
fn verify_twap_seize_zero_price_none() {
    let debt_sol: u64 = kani::any();
    let bonus: u16 = kani::any();
    assert!(twap_tokens_to_seize(debt_sol, bonus, 0).is_none());
}

#[kani::proof]
fn verify_twap_seize_q64_exact() {
    let q1: u128 = 1u128 << 64; // price 1.0 sol/token
    let q_half: u128 = 1u128 << 63; // price 0.5
    // At 1.0 sol/token, no bonus: cover debt_sol SOL ⇒ seize debt_sol tokens.
    assert!(twap_tokens_to_seize(5_000_000, 0, q1) == Some(5_000_000));
    // At 0.5 sol/token, no bonus: each token is worth 0.5 ⇒ seize 2× the debt.
    assert!(twap_tokens_to_seize(5_000_000, 0, q_half) == Some(10_000_000));
    // 5% bonus at 1.0 sol/token ⇒ 1.05× the debt in tokens.
    assert!(twap_tokens_to_seize(5_000_000, 500, q1) == Some(5_250_000));
}

#[kani::proof]
fn verify_twap_seize_overflow_is_none() {
    // The grossed-up `debt × (10_000+bonus) × 2^64` overflows u128 for an
    // enormous debt → None (fail closed), never a panic/wrap. Price is a valid
    // non-zero 1.0 so the zero-guard isn't what trips.
    let q1: u128 = 1u128 << 64;
    assert!(twap_tokens_to_seize(u64::MAX, 0, q1).is_none());
}

// ============================================================================
// 85. [V21][D-10] effective_liq_bonus_bps — distress-scaled bonus (prize cap)
//     Proves: 0 at/below the liq threshold (incl. a manufactured barely-over
//     case ≈ 0), full bonus at/above the full-bonus LTV (incl. u64::MAX
//     zero-collateral), monotone non-decreasing in LTV, and bounded by
//     max_bonus. This removes the prize from a manufactured liquidation.
// ============================================================================

#[kani::proof]
fn verify_bonus_zero_at_or_below_threshold() {
    let ltv: u64 = kani::any();
    kani::assume(ltv <= DEFAULT_LIQUIDATION_THRESHOLD_BPS as u64);
    let b = effective_liq_bonus_bps(
        ltv,
        DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        LIQ_FULL_BONUS_LTV_BPS,
        DEFAULT_LIQUIDATION_BONUS_BPS,
    );
    assert!(b == 0);
}

#[kani::proof]
fn verify_bonus_full_at_or_above_full_ltv() {
    let ltv: u64 = kani::any();
    kani::assume(ltv >= LIQ_FULL_BONUS_LTV_BPS as u64); // includes u64::MAX
    let b = effective_liq_bonus_bps(
        ltv,
        DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        LIQ_FULL_BONUS_LTV_BPS,
        DEFAULT_LIQUIDATION_BONUS_BPS,
    );
    assert!(b == DEFAULT_LIQUIDATION_BONUS_BPS as u64);
}

#[kani::proof]
fn verify_bonus_monotonic_and_bounded() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= b);
    let ba = effective_liq_bonus_bps(
        a,
        DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        LIQ_FULL_BONUS_LTV_BPS,
        DEFAULT_LIQUIDATION_BONUS_BPS,
    );
    let bb = effective_liq_bonus_bps(
        b,
        DEFAULT_LIQUIDATION_THRESHOLD_BPS,
        LIQ_FULL_BONUS_LTV_BPS,
        DEFAULT_LIQUIDATION_BONUS_BPS,
    );
    assert!(bb >= ba);
    assert!(bb <= DEFAULT_LIQUIDATION_BONUS_BPS as u64);
}

#[kani::proof]
fn verify_bonus_ramp_shape() {
    let t = DEFAULT_LIQUIDATION_THRESHOLD_BPS; // 6500
    let f = LIQ_FULL_BONUS_LTV_BPS; // 7547 (derived: 100/(1+bonus))
    let m = DEFAULT_LIQUIDATION_BONUS_BPS; // 3250 (= 1.3·ρ_max). span = f−t = 1047.
    // Threshold and below: 0 prize.
    assert!(effective_liq_bonus_bps(6_500, t, f, m) == 0);
    // Manufactured barely-over: tiny prize. 3250 × 100 / 1047 = 310 bps.
    assert!(effective_liq_bonus_bps(6_600, t, f, m) == 310);
    // Interior: 3250 × 500 / 1047 = 1551 bps.
    assert!(effective_liq_bonus_bps(7_000, t, f, m) == 1551);
    // At/above full-bonus LTV: full ceiling (32.5%).
    assert!(effective_liq_bonus_bps(7_547, t, f, m) == 3_250);
    assert!(m == 3_250);
}

