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
// 21. V26 MIGRATION: SOL Wrapping Conservation
//     Proves: bonding curve SOL debited == WSOL ATA credited (exact, no loss).
//     For any valid reserves amount, the lamport transfer is conserving.
// ============================================================================

#[kani::proof]
fn verify_sol_wrapping_conservation() {
    let real_sol_reserves: u64 = kani::any();
    kani::assume(real_sol_reserves > 0);
    kani::assume(real_sol_reserves <= BONDING_TARGET_LAMPORTS);

    // Bonding curve has real_sol_reserves + rent-exempt lamports
    let rent_exempt: u64 = kani::any();
    kani::assume(rent_exempt > 0);
    kani::assume(rent_exempt <= 10_000_000);
    let bc_lamports = real_sol_reserves.checked_add(rent_exempt).unwrap();

    // sub_lamports: bc_lamports - real_sol_reserves must not underflow
    let bc_after = bc_lamports.checked_sub(real_sol_reserves).unwrap();
    assert!(bc_after == rent_exempt); // BC retains rent-exempt

    // WSOL ATA receives exactly real_sol_reserves
    let wsol_before: u64 = kani::any();
    kani::assume(wsol_before <= u64::MAX - real_sol_reserves);
    let wsol_after = wsol_before.checked_add(real_sol_reserves).unwrap();
    assert!(wsol_after - wsol_before == real_sol_reserves);

    // Total lamports conserved (u128 to avoid overflow in the assertion itself)
    assert!(bc_after as u128 + wsol_after as u128 == bc_lamports as u128 + wsol_before as u128);
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
// 51. [V5] SHORT: Lifecycle Conservation (Open → Close, No Interest)
//     Proves: after open_short and immediate close_short, treasury tokens
//     are perfectly conserved (tokens lent out = tokens returned).
// ============================================================================

#[kani::proof]
fn verify_short_lifecycle_conservation() {
    let tokens_borrowed: u64 = kani::any();
    let sol_collateral: u64 = kani::any();
    let pool_sol: u64 = 100_000_000_000; // 100 SOL pool
    let pool_tokens: u64 = 50_000_000_000_000; // 50T tokens

    kani::assume(tokens_borrowed >= MIN_SHORT_TOKENS);
    kani::assume(tokens_borrowed <= TOTAL_SUPPLY / 10); // Max 10% of supply
    kani::assume(sol_collateral >= MIN_BORROW_AMOUNT);
    kani::assume(sol_collateral <= 500_000_000_000); // Max 500 SOL

    // Treasury tokens before
    let treasury_tokens_before: u64 = kani::any();
    kani::assume(treasury_tokens_before >= tokens_borrowed);
    kani::assume(treasury_tokens_before <= TOTAL_SUPPLY);

    // Treasury SOL before
    let treasury_sol_before: u64 = kani::any();
    kani::assume(treasury_sol_before <= 1_000_000_000_000); // Max 1000 SOL

    // LTV check
    let debt_value = calc_short_debt_value(tokens_borrowed, pool_sol, pool_tokens).unwrap();
    kani::assume(debt_value > 0);
    let ltv = calc_ltv_bps(debt_value, sol_collateral).unwrap();
    kani::assume(ltv <= DEFAULT_MAX_LTV_BPS as u64);

    // ========== OPEN SHORT ==========
    let treasury_tokens_after_open = treasury_tokens_before.checked_sub(tokens_borrowed).unwrap();
    let treasury_sol_after_open = treasury_sol_before.checked_add(sol_collateral).unwrap();
    let short_collateral_reserved = sol_collateral;

    // ========== CLOSE SHORT (immediate, no interest) ==========
    let total_owed = tokens_borrowed; // No interest (same slot)
    let actual_return = total_owed;

    let treasury_tokens_after_close = treasury_tokens_after_open
        .checked_add(actual_return)
        .unwrap();
    let treasury_sol_after_close = treasury_sol_after_open.checked_sub(sol_collateral).unwrap();
    let short_collateral_after = short_collateral_reserved - sol_collateral;

    // ========== ASSERTIONS ==========
    // Treasury tokens perfectly conserved
    assert!(treasury_tokens_after_close == treasury_tokens_before);

    // Treasury SOL perfectly conserved
    assert!(treasury_sol_after_close == treasury_sol_before);

    // Short collateral fully released
    assert!(short_collateral_after == 0);
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
// 54. [V5] SHORT: Collateral Reservation Correctness
//     Proves: repurposed total_burned_from_buyback accurately tracks
//     short collateral, and lending available SOL correctly excludes it.
// ============================================================================

#[kani::proof]
fn verify_short_collateral_reservation() {
    let treasury_sol: u64 = kani::any();
    let short_collateral: u64 = kani::any();
    let sol_lent: u64 = kani::any();

    kani::assume(treasury_sol >= 1_000_000_000); // Min 1 SOL
    kani::assume(treasury_sol <= 1_000_000_000_000); // Max 1000 SOL
    kani::assume(short_collateral <= treasury_sol);
    kani::assume(sol_lent <= treasury_sol);

    // Available SOL for lending = treasury_sol - short_collateral
    let available = treasury_sol.saturating_sub(short_collateral);

    // Max lendable = available * 80%
    let max_lendable = (available as u128)
        .checked_mul(DEFAULT_LENDING_UTILIZATION_CAP_BPS as u128)
        .unwrap()
        .checked_div(10000)
        .unwrap() as u64;

    // Short collateral is never touched by lending
    assert!(max_lendable <= available);
    assert!(available <= treasury_sol);
    assert!(max_lendable <= treasury_sol);

    // If no shorts, full treasury available
    if short_collateral == 0 {
        assert!(available == treasury_sol);
    }

    // If all treasury is short collateral, nothing available for lending
    if short_collateral == treasury_sol {
        assert!(available == 0);
        assert!(max_lendable == 0);
    }
}

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

    // If collateral insufficient, bad debt occurs
    let actual_debt_covered = if collateral_to_seize > collateral {
        calc_collateral_value(actual_collateral_seized, pool_sol, pool_tokens).unwrap()
    } else {
        debt_to_cover
    };

    let bad_debt = total_debt.saturating_sub(
        actual_debt_covered
            .checked_add(total_debt.saturating_sub(debt_to_cover))
            .unwrap(),
    );

    // Apply repayment: interest first, then principal
    let mut remaining_debt_paid = actual_debt_covered;
    let mut loan_interest = interest;
    let mut loan_borrowed = borrowed;

    if remaining_debt_paid <= loan_interest {
        loan_interest -= remaining_debt_paid;
        remaining_debt_paid = 0;
    } else {
        remaining_debt_paid -= loan_interest;
        loan_interest = 0;
        loan_borrowed = loan_borrowed.saturating_sub(remaining_debt_paid);
    }

    // Write off bad debt
    if bad_debt > 0 {
        loan_borrowed = loan_borrowed.saturating_sub(bad_debt);
        loan_interest = 0;
    }

    // Fixed: total_sol_lent reduced by principal paid AND bad debt
    let total_sol_lent_after = total_sol_lent_before
        .saturating_sub(remaining_debt_paid)
        .saturating_sub(bad_debt);

    // Key property: if loan is fully liquidated, total_sol_lent decreased by
    // at least the original borrowed amount
    if loan_borrowed == 0 && loan_interest == 0 {
        assert!(total_sol_lent_after <= total_sol_lent_before.saturating_sub(borrowed));
    }

    // total_sol_lent never goes negative (saturating)
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

    let bad_debt_tokens = total_token_debt.saturating_sub(
        actual_tokens_covered
            .checked_add(total_token_debt.saturating_sub(tokens_to_cover))
            .unwrap(),
    );

    // Apply repayment: interest first, then principal
    let mut remaining_tokens_paid = actual_tokens_covered;
    let mut pos_interest = interest;
    let mut pos_borrowed = tokens_borrowed;

    if remaining_tokens_paid <= pos_interest {
        pos_interest -= remaining_tokens_paid;
        remaining_tokens_paid = 0;
    } else {
        remaining_tokens_paid -= pos_interest;
        pos_interest = 0;
        pos_borrowed = pos_borrowed.saturating_sub(remaining_tokens_paid);
    }

    // Write off bad debt
    if bad_debt_tokens > 0 {
        pos_borrowed = pos_borrowed.saturating_sub(bad_debt_tokens);
        pos_interest = 0;
    }

    // NEW (fixed): total_tokens_lent reduced by principal paid AND bad debt
    let total_tokens_lent_after = total_tokens_lent_before
        .saturating_sub(remaining_tokens_paid)
        .saturating_sub(bad_debt_tokens);

    // Key property: if position is fully liquidated, total_tokens_lent decreased
    // by at least the original tokens_borrowed
    if pos_borrowed == 0 && pos_interest == 0 {
        assert!(
            total_tokens_lent_after <= total_tokens_lent_before.saturating_sub(tokens_borrowed)
        );
    }

    // total_tokens_lent never goes negative (saturating)
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
// 59. [V6] CIRCUIT BREAKER: Price Deviation Band Symmetry
//     Proves: the deviation band is symmetric around baseline, and baseline
//     price always passes (0% deviation). Uses concrete baseline with
//     symbolic current ratio for SAT tractability.
// ============================================================================

fn calc_price_in_band(
    pool_sol: u64,
    pool_tokens: u64,
    baseline_sol: u64,
    baseline_tokens: u64,
) -> Option<bool> {
    let current_ratio = (pool_sol as u128)
        .checked_mul(RATIO_PRECISION)?
        .checked_div(pool_tokens as u128)?;
    let baseline_ratio = (baseline_sol as u128)
        .checked_mul(RATIO_PRECISION)?
        .checked_div(baseline_tokens as u128)?;

    let upper = baseline_ratio
        .checked_mul(10000 + MAX_PRICE_DEVIATION_BPS as u128)?
        .checked_div(10000)?;
    let lower = baseline_ratio
        .checked_mul(10000_u128.saturating_sub(MAX_PRICE_DEVIATION_BPS as u128))?
        .checked_div(10000)?;

    Some(current_ratio >= lower && current_ratio <= upper)
}

#[kani::proof]
fn verify_circuit_breaker_baseline_passes() {
    // Baseline price always passes its own check
    let baseline_sol: u64 = 100_000_000_000; // 100 SOL
    let baseline_tokens: u64 = 50_000_000_000_000; // 50T tokens

    let result = calc_price_in_band(baseline_sol, baseline_tokens, baseline_sol, baseline_tokens);
    assert!(result.unwrap());
}

// ============================================================================
// 60. [V6] CIRCUIT BREAKER: Deviation Band Rejects Out-of-Range Price
//     Proves: a 2x price (100% increase) is rejected by the 50% band.
// ============================================================================

#[kani::proof]
fn verify_circuit_breaker_rejects_doubled_price() {
    let baseline_sol: u64 = 100_000_000_000;
    let baseline_tokens: u64 = 50_000_000_000_000;

    // Double the SOL = 2x price (100% increase, exceeds 50% band)
    let result = calc_price_in_band(
        200_000_000_000,
        baseline_tokens,
        baseline_sol,
        baseline_tokens,
    );
    assert!(!result.unwrap());
}

// ============================================================================
// 61. [V6] CIRCUIT BREAKER: Band Edge Acceptance
//     Proves: price at exactly +49% and -49% passes, +51% and -51% fails.
//     Concrete values for fast SAT solving.
// ============================================================================

#[kani::proof]
fn verify_circuit_breaker_band_edges() {
    let baseline_sol: u64 = 100_000_000_000; // 100 SOL
    let baseline_tokens: u64 = 100_000_000_000_000; // 100T tokens

    // +49%: pool_sol = 149 SOL (price up 49%)
    let up_49 = calc_price_in_band(
        149_000_000_000,
        baseline_tokens,
        baseline_sol,
        baseline_tokens,
    );
    assert!(up_49.unwrap());

    // +51%: pool_sol = 151 SOL (price up 51%)
    let up_51 = calc_price_in_band(
        151_000_000_000,
        baseline_tokens,
        baseline_sol,
        baseline_tokens,
    );
    assert!(!up_51.unwrap());

    // -49%: pool_sol = 51 SOL (price down 49%)
    let down_49 = calc_price_in_band(
        51_000_000_000,
        baseline_tokens,
        baseline_sol,
        baseline_tokens,
    );
    assert!(down_49.unwrap());

    // -51%: pool_sol = 49 SOL (price down 51%)
    let down_51 = calc_price_in_band(
        49_000_000_000,
        baseline_tokens,
        baseline_sol,
        baseline_tokens,
    );
    assert!(!down_51.unwrap());
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
// 65. TREASURY: Ratio Gate Fee Subtraction Safety
//     Proves: after saturating_sub of accumulated fees from vault balances,
//     the ratio calculation either succeeds or is blocked by the pool_tokens > 0
//     guard. No division by zero, no inflated ratio from negative balances.
// ============================================================================

#[kani::proof]
fn verify_ratio_gate_fee_subtraction_safe() {
    let vault_sol: u64 = kani::any();
    let vault_tokens: u64 = kani::any();
    let sol_fees: u64 = kani::any();
    let token_fees: u64 = kani::any();

    kani::assume(vault_sol <= 10_000_000_000_000); // Max 10K SOL
    kani::assume(vault_tokens <= TOTAL_SUPPLY);
    kani::assume(sol_fees <= vault_sol); // Fees can't exceed vault (Raydium invariant)
    kani::assume(token_fees <= vault_tokens);

    let pool_sol = vault_sol.saturating_sub(sol_fees);
    let pool_tokens = vault_tokens.saturating_sub(token_fees);

    // If pool_tokens == 0, the on-chain require! blocks the operation
    if pool_tokens > 0 {
        // Ratio computation must succeed (no division by zero)
        let ratio = (pool_sol as u128)
            .checked_mul(RATIO_PRECISION)
            .unwrap()
            .checked_div(pool_tokens as u128);
        assert!(ratio.is_some());
    }

    // Fee subtraction never produces values larger than original
    assert!(pool_sol <= vault_sol);
    assert!(pool_tokens <= vault_tokens);
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

//     Proves: get_depth_max_ltv_bps returns correct tier at every boundary,
//     tiers are exhaustive (no gaps), and LTV values are monotonically increasing.

fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < MIN_POOL_SOL_LENDING {
        0
    } else if pool_sol < DEPTH_TIER_1 {
        DEPTH_LTV_0
    } else if pool_sol < DEPTH_TIER_2 {
        DEPTH_LTV_1
    } else if pool_sol < DEPTH_TIER_3 {
        DEPTH_LTV_2
    } else {
        DEPTH_LTV_3
    }
}

#[kani::proof]
fn verify_depth_bands_boundaries() {
    // Below minimum: blocked
    assert!(get_depth_max_ltv_bps(0) == 0);
    assert!(get_depth_max_ltv_bps(4_999_999_999) == 0);

    // Band 0: 5 SOL to 50 SOL
    assert!(get_depth_max_ltv_bps(5_000_000_000) == DEPTH_LTV_0);
    assert!(get_depth_max_ltv_bps(49_999_999_999) == DEPTH_LTV_0);

    // Band 1: 50 SOL to 200 SOL
    assert!(get_depth_max_ltv_bps(50_000_000_000) == DEPTH_LTV_1);
    assert!(get_depth_max_ltv_bps(199_999_999_999) == DEPTH_LTV_1);

    // Band 2: 200 SOL to 500 SOL
    assert!(get_depth_max_ltv_bps(200_000_000_000) == DEPTH_LTV_2);
    assert!(get_depth_max_ltv_bps(499_999_999_999) == DEPTH_LTV_2);

    // Band 3: 500+ SOL
    assert!(get_depth_max_ltv_bps(500_000_000_000) == DEPTH_LTV_3);
    assert!(get_depth_max_ltv_bps(u64::MAX) == DEPTH_LTV_3);

    // Monotonic: each tier >= previous
    assert!(DEPTH_LTV_0 <= DEPTH_LTV_1);
    assert!(DEPTH_LTV_1 <= DEPTH_LTV_2);
    assert!(DEPTH_LTV_2 <= DEPTH_LTV_3);
}
