// Lending tests (long): borrow / repay / liquidate.
//
// 22 tests covering every reachable variant in the lending path.
// Setup notes:
//   - FLAME bonding gives pool ~100 SOL / 150M tokens post-migration.
//   - First buyer holds ~19M tokens (just under wallet cap) — ideal borrower.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signature::Keypair, signer::Signer};

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

// `migrated()` — bare post-migration state. Three-period fixture model:
//
//   1. bonding   — pre-migration, curve incomplete (each test scaffolds inline)
//   2. migrated  — post-migration, treasury naturally below lending gate
//                  (~5 SOL from bond fees). Tests SHORTS (work from migration)
//                  and tests that LENDING REFUSES (gate fires).
//   3. lending   — post-migration + treasury manually pumped above gate.
//                  Tests lending/margin paths that need the gate cleared.
//                  See `lending_ready()`.
fn migrated() -> (Env, TokenCtx, Keypair) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let borrower = env.bond_to_completion(&t);
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");
    env.airdrop(&borrower.pubkey(), 5 * LAMPORTS_PER_SOL); // top up post-bonding
    (env, t, borrower)
}

// `lending_ready()` — migrated + lending unlocked. The fixture for tests
// that exercise borrow/repay/liquidate paths.
//
// Three pokes layered on top of `migrated()`:
//
//   1. Treasury sol_balance = 200 SOL. Clears the mainnet 100 SOL gate
//      AND leaves headroom under the 20% absolute per-user cap
//      (max_lendable = 160 SOL → per-user cap = 32 SOL).
//
//   2. Pool SOL = 500 SOL. Puts the pool in depth_tier_3 (45% max LTV).
//
//   3. Pool token vault = 100M tokens (1e14 micro-units). Fixes pool price
//      at 5e-6 SOL/token regardless of what migration left behind.
//      Combined with the first buyer's natural ~19M-token balance, gives
//      ~95 SOL of collateral value, allowing ~42 SOL of LTV-permitted
//      borrowing — enough headroom to test the per-user cap firing
//      without LTV firing first.
//
// Tests that need a specific gate-fail or pool-thin state poke the
// relevant field BACK DOWN explicitly (see `borrow_lending_not_yet_unlocked`,
// `borrow_lending_cap_exceeded`, `liquidate_via_vault_happy`).
//
// Real bond-completion fees only seed a few SOL into treasury, well below
// the gate. Production tokens build up to lending-unlocked via sustained
// trading volume + transfer fees + harvest+swap. The pokes here simulate
// that endpoint without the volume cost. An end-to-end natural-unlock test
// would be a useful complement (TODO).
fn lending_ready() -> (Env, TokenCtx, Keypair) {
    let (mut env, t, borrower) = migrated();
    let mut tr = env.get_treasury(&t);
    tr.sol_balance = 200 * LAMPORTS_PER_SOL;
    env.poke_anchor(t.treasury, tr);
    env.poke_pool_sol(&t, 500 * LAMPORTS_PER_SOL);
    env.poke_token_amount(t.deep_pool_token_vault, 100_000_000_000_000); // 100M tokens
    (env, t, borrower)
}

fn token_balance(
    env: &Env,
    owner: &solana_sdk::pubkey::Pubkey,
    mint: &solana_sdk::pubkey::Pubkey,
) -> u64 {
    use solana_sdk::account::ReadableAccount;
    use torch_market::token_2022_utils::get_associated_token_address_2022;
    let ata = get_associated_token_address_2022(owner, mint);
    let acct = env.svm.get_account(&ata).expect("ATA missing");
    u64::from_le_bytes(acct.data()[64..72].try_into().unwrap())
}

// ============================================================================
// Borrow (9)
// ============================================================================

#[test]
fn borrow_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let borrow_amount = 500_000_000; // 0.5 SOL — well within all caps
    env.borrow(&borrower, &t, bal / 2, borrow_amount)
        .expect("borrow");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan exists");
    assert_eq!(loan.borrowed_amount, borrow_amount);
    assert!(loan.collateral_amount > 0);
    let tr = env.get_treasury(&t);
    assert_eq!(tr.total_sol_lent, borrow_amount);
    assert_eq!(tr.active_loans, 1);
}

#[test]
fn borrow_lending_not_enabled() {
    let (mut env, t, borrower) = lending_ready();
    let mut tr = env.get_treasury(&t);
    tr.lending_enabled = false;
    env.poke_anchor(t.treasury, tr);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal / 2, 500_000_000),
        TorchMarketError::LendingNotEnabled
    );
}

#[test]
fn borrow_lending_requires_migration() {
    // Not migrated.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed");

    let bal = token_balance(&env, &buyer.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&buyer, &t, bal / 2, 500_000_000),
        TorchMarketError::LendingRequiresMigration
    );
}

#[test]
fn borrow_ltv_exceeded() {
    // Bare migrated() — needs the LEAN post-migration pool state for the
    // LTV math to fire on a small collateral. With lending_ready()'s
    // pumped pool, 1M tokens would be worth ~5 SOL and 1 SOL borrow
    // would pass LTV. Stays on migrated() where 1M tokens ≈ 0.67 SOL.
    let (mut env, t, borrower) = migrated();
    expect_err!(
        env.borrow(&borrower, &t, 1_000_000, 1_000_000_000),
        TorchMarketError::LtvExceeded
    );
}

#[test]
#[cfg(not(feature = "simnet"))]
fn borrow_lending_not_yet_unlocked() {
    // Verify the gate fires when treasury is below the unlock threshold.
    // Bare migrated() naturally leaves ~5 SOL of bond fees in treasury,
    // which is below the mainnet 100 SOL gate but ABOVE the devnet 1 SOL
    // gate. Poke treasury to 0.5 SOL to ensure we're below both gates.
    // Skipped on `simnet` builds because there the threshold is 0 (gate
    // never fires).
    let (mut env, t, borrower) = migrated();
    let mut tr = env.get_treasury(&t);
    tr.sol_balance = 500_000_000; // 0.5 SOL — below both devnet (1) and mainnet (100) gates
    env.poke_anchor(t.treasury, tr);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal, 100_000_000),
        TorchMarketError::LendingNotYetUnlocked
    );
}

#[test]
fn borrow_lending_cap_exceeded() {
    // Production-realistic: gate cleared, but treasury's lending utilization
    // cap is exhausted because total_sol_lent is already near max_lendable.
    // We simulate that by poking lending_utilization_cap_bps to 1 bp so
    // max_lendable = 200 SOL × 0.01% = 0.02 SOL. A 0.5 SOL borrow attempt
    // tips total_lent past max_lendable → fires LendingCapExceeded.
    //
    // The deeper-pool collateral (set in migrated()) lets the 0.5 SOL
    // borrow pass the LTV gate, so this test isolates the utilization
    // cap from other concerns.
    let (mut env, t, borrower) = lending_ready();
    let mut tr = env.get_treasury(&t);
    tr.lending_utilization_cap_bps = 1; // 0.01% → max_lendable ≈ 0.02 SOL
    env.poke_anchor(t.treasury, tr);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal, 500_000_000),
        TorchMarketError::LendingCapExceeded
    );
}

#[test]
fn borrow_user_cap_exceeded() {
    // Production-realistic: with the deeper-pool migrated() setup, LTV is
    // permissive enough to allow a borrow > the absolute 20% per-user cap
    // (= max_lendable × 20% = 160 × 0.2 = 32 SOL). Borrow 33 SOL: LTV
    // passes (collateral_value ≈ 80 SOL × 45% LTV = 36 SOL allowed), but
    // 33 SOL > 32 SOL per-user cap → fires UserBorrowCapExceeded.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal, 33 * LAMPORTS_PER_SOL),
        TorchMarketError::UserBorrowCapExceeded
    );
}

#[test]
fn borrow_below_min_amount() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    // sol_to_borrow > 0 but below MIN_BORROW_AMOUNT (0.1 SOL).
    expect_err!(
        env.borrow(&borrower, &t, bal, MIN_BORROW_AMOUNT - 1),
        TorchMarketError::BorrowTooSmall
    );
}

#[test]
fn borrow_partial_deposit_only() {
    // collateral > 0, sol_to_borrow = 0. Handler skips the borrow branch.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let deposit = bal / 3;
    env.borrow(&borrower, &t, deposit, 0).expect("deposit-only");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan exists");
    assert_eq!(loan.borrowed_amount, 0);
    assert!(loan.collateral_amount > 0);
}

#[test]
fn borrow_pool_too_thin_blocks_new_position() {
    // Pool < MIN_POOL_SOL_LENDING (5 SOL after rent_exempt overhead) → depth
    // tier returns 0, check_borrow_ltv raises PoolTooThin. New positions blocked
    // at thin pools, even though liquidations of existing positions are allowed.
    // Bare migrated() — lending_ready() would have a pumped pool that
    // contradicts the "thin pool" we're trying to test.
    let (mut env, t, borrower) = migrated();
    env.poke_pool_sol(&t, MIN_POOL_SOL_LENDING - 1);
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal / 2, 200_000_000),
        TorchMarketError::PoolTooThin
    );
}

#[test]
fn borrow_depth_tier_zero_caps_ltv_lower() {
    // Poke deep_pool to tier 0 (≥5 SOL, <50 SOL → DEPTH_LTV_0 = 25%).
    // A borrow that would pass at tier 1 (35%) must fail here.
    // Bare migrated() — this test relies on the natural pool_tokens left
    // by migration (~149.78M per comment below), since lending_ready()
    // overrides token vault to 100M which would skew the LTV math.
    let (mut env, t, borrower) = migrated();
    env.poke_pool_sol(&t, 30 * LAMPORTS_PER_SOL); // 30 SOL → tier 0

    // At pool=30 SOL, pool_tokens=149.78M, ~16.19M tokens collateral:
    //   collateral_value ≈ 16.19M * 30 / 149.78M = 3.24 SOL
    //   tier 0 max debt = 3.24 * 0.25 = 0.81 SOL
    //   tier 1 max debt = 3.24 * 0.35 = 1.13 SOL
    // Borrowing 1 SOL exceeds tier 0 (25%) but would fit at tier 1 (35%).
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.borrow(&borrower, &t, bal, LAMPORTS_PER_SOL),
        TorchMarketError::LtvExceeded
    );
}

#[test]
fn borrow_via_vault_happy() {
    let (mut env, t, borrower) = lending_ready();
    // borrower already has tokens from bonding; create vault, link borrower,
    // move tokens into vault ATA, borrow via vault.
    let vault_owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&vault_owner);
    env.link_wallet(&vault_owner, &vault, borrower.pubkey())
        .expect("link");

    // Move borrower's tokens into vault ATA via withdraw_tokens... actually easier:
    // do the borrow with the BORROWER's tokens. But borrow_via_vault transfers
    // from vault_token_account. So tokens must be in vault. Simplest path: have
    // a vault-linked buyer do a fresh buy via vault (post-migration via deep_pool,
    // which we don't have a helper for). Skip — borrow direct from vault using
    // an already-vault-held token balance is non-trivial without a swap helper.
    //
    // Instead: use vault for the borrow ix but pass collateral_amount=0 to skip
    // the token transfer. Borrow happens, SOL goes to vault.
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, 0)
        .expect("deposit collateral first");
    // Now collateral is in collateral_vault under borrower's loan_position.
    // borrow_via_vault with same borrower → adds 0 collateral, borrows SOL to vault.
    env.deposit_vault(&vault_owner, &vault, 100_000_000)
        .expect("seed vault rent");
    env.borrow_via_vault(&borrower, &vault, &t, 0, 200_000_000)
        .expect("borrow_via_vault");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan");
    assert_eq!(loan.borrowed_amount, 200_000_000);
    let v = env.get_torch_vault(&vault.vault);
    assert!(v.sol_balance > 100_000_000); // initial deposit + borrowed SOL
    assert!(v.total_received >= 200_000_000);
}

// ============================================================================
// Repay (6)
// ============================================================================

#[test]
fn repay_partial() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, 500_000_000)
        .expect("borrow");

    env.repay(&borrower, &t, 200_000_000)
        .expect("partial repay");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan");
    // 0 slots elapsed → no interest accrued. All payment goes to principal.
    assert_eq!(loan.borrowed_amount, 300_000_000);
    assert_eq!(loan.accrued_interest, 0);
}

#[test]
fn repay_full_returns_collateral() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let coll_before = bal;
    env.borrow(&borrower, &t, bal / 2, 500_000_000)
        .expect("borrow");
    let after_borrow = token_balance(&env, &borrower.pubkey(), &t.mint);
    assert!(after_borrow < coll_before);

    // Send way more than owed — handler clamps to total_owed.
    env.repay(&borrower, &t, 10 * LAMPORTS_PER_SOL)
        .expect("full repay");

    // Tier B: loan PDA closed on full repay.
    assert!(
        env.get_loan(&t, &borrower.pubkey()).is_none(),
        "loan PDA closed"
    );

    let after_repay = token_balance(&env, &borrower.pubkey(), &t.mint);
    assert!(
        after_repay > after_borrow,
        "collateral returned to borrower"
    );

    let tr = env.get_treasury(&t);
    assert_eq!(tr.active_loans, 0);
}

#[test]
fn repay_interest_first() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, LAMPORTS_PER_SOL)
        .expect("borrow"); // 1 SOL

    // Warp slots to accrue interest. At 200 bps/epoch on 1 SOL: ~33 lamports/slot.
    // 100k slots → ~3.3M lamports ≈ 0.0033 SOL interest.
    env.warp_to_slot(env.current_slot() + 100_000);

    // Repay 0.001 SOL — entirely interest, no principal touched.
    env.repay(&borrower, &t, 1_000_000)
        .expect("interest-only repay");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan");
    assert_eq!(
        loan.borrowed_amount, LAMPORTS_PER_SOL,
        "principal untouched"
    );
    // accrued_interest = accrued - 1M (small positive remainder)
    assert!(loan.accrued_interest > 0);
    assert!(loan.accrued_interest < 5_000_000);
}

#[test]
fn repay_no_active_loan() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    // Open a deposit-only "loan" (borrowed_amount = 0). Then try to repay.
    env.borrow(&borrower, &t, bal / 2, 0).expect("deposit only");

    // Context constraint: `loan_position.borrowed_amount > 0`.
    expect_err!(
        env.repay(&borrower, &t, 100_000_000),
        TorchMarketError::NoActiveLoan
    );
}

#[test]
fn repay_zero_amount() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, 500_000_000)
        .expect("borrow");

    expect_err!(env.repay(&borrower, &t, 0), TorchMarketError::ZeroAmount);
}

#[test]
fn repay_via_vault_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, 500_000_000)
        .expect("borrow");

    // Vault holds SOL to repay with.
    let vault_owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&vault_owner);
    env.link_wallet(&vault_owner, &vault, borrower.pubkey())
        .expect("link");
    env.deposit_vault(&vault_owner, &vault, LAMPORTS_PER_SOL)
        .expect("fund vault");

    env.repay_via_vault(&borrower, &vault, &t, 200_000_000)
        .expect("repay_via_vault");

    let loan = env.get_loan(&t, &borrower.pubkey()).expect("loan");
    assert_eq!(loan.borrowed_amount, 300_000_000);

    let v = env.get_torch_vault(&vault.vault);
    assert_eq!(v.total_spent, 200_000_000);
}

// ============================================================================
// Liquidate (5)
// ============================================================================

#[test]
fn liquidate_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    // Open a near-max LTV loan, then drop pool to push it underwater.
    env.borrow(&borrower, &t, bal, 2 * LAMPORTS_PER_SOL)
        .expect("borrow");

    // Pool sol drop: 500 SOL → 7 SOL. With lending_ready()'s fixed
    // pool_tokens = 100M, collateral_value = 19M × 7/100M ≈ 1.33 SOL.
    // Debt 2 SOL → LTV ≈ 150% >> liquidation_threshold (65%).
    // 7 SOL > MIN_POOL_SOL_LENDING (5 SOL), so liquidate isn't blocked.
    env.poke_pool_sol(&t, 7 * LAMPORTS_PER_SOL);

    let liquidator = env.new_funded(5 * LAMPORTS_PER_SOL);
    env.liquidate(&liquidator, borrower.pubkey(), &t)
        .expect("liquidate");

    let loan = env
        .get_loan(&t, &borrower.pubkey())
        .expect("loan still exists");
    // Partial liquidation at default close_bps=50%: debt covered halved.
    assert!(loan.borrowed_amount < 2 * LAMPORTS_PER_SOL);
}

#[test]
fn liquidate_not_liquidatable() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 2, 200_000_000)
        .expect("conservative borrow"); // 0.2 SOL on ~6 SOL collateral → ~3% LTV

    // No pool manipulation — position is healthy.
    let liquidator = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.liquidate(&liquidator, borrower.pubkey(), &t),
        TorchMarketError::NotLiquidatable
    );
}

#[test]
fn liquidate_partial_capped_at_close_bps() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal, 2 * LAMPORTS_PER_SOL)
        .expect("borrow");

    let before = env.get_loan(&t, &borrower.pubkey()).unwrap();
    env.poke_pool_sol(&t, 7 * LAMPORTS_PER_SOL);

    let liquidator = env.new_funded(5 * LAMPORTS_PER_SOL);
    env.liquidate(&liquidator, borrower.pubkey(), &t)
        .expect("liquidate");

    let after = env.get_loan(&t, &borrower.pubkey()).unwrap();
    let debt_covered = before.borrowed_amount - after.borrowed_amount;
    // close_bps = 50%, so at most 50% of total debt is covered in one call.
    assert!(debt_covered <= before.borrowed_amount / 2 + 1_000_000);
    assert!(
        after.borrowed_amount > 0,
        "position still has debt after partial"
    );
}

#[test]
fn liquidate_full_with_bad_debt() {
    // Crush the pool so the value of all collateral is less than current debt:
    // → seizure takes all collateral, leaves bad_debt > 0, position cleared.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal, 2 * LAMPORTS_PER_SOL)
        .expect("borrow");
    // Pool drop: 100 SOL → 6 SOL (above MIN_POOL_SOL_LENDING after rent_exempt).
    // Collateral value at that ratio ≈ 0.65 SOL, way below 2 SOL debt × 1.1 bonus.
    env.poke_pool_sol(&t, 6 * LAMPORTS_PER_SOL);

    let liquidator = env.new_funded(5 * LAMPORTS_PER_SOL);
    env.liquidate(&liquidator, borrower.pubkey(), &t)
        .expect("liquidate");

    let loan = env.get_loan(&t, &borrower.pubkey()).unwrap();
    // Bad-debt branch zeroes out remaining debt + interest after seizing all collateral.
    assert_eq!(loan.collateral_amount, 0);
    // borrowed_amount and accrued_interest are zeroed by bad_debt cleanup.
    assert_eq!(loan.accrued_interest, 0);
}

#[test]
fn liquidate_via_vault_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal, 2 * LAMPORTS_PER_SOL)
        .expect("borrow");
    env.poke_pool_sol(&t, 7 * LAMPORTS_PER_SOL);

    let liquidator_wallet = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&liquidator_wallet);
    env.deposit_vault(&liquidator_wallet, &vault, 3 * LAMPORTS_PER_SOL)
        .expect("fund vault");

    env.liquidate_via_vault(&liquidator_wallet, &vault, borrower.pubkey(), &t)
        .expect("liquidate_via_vault");

    let v = env.get_torch_vault(&vault.vault);
    assert!(v.total_spent > 0); // vault paid the debt-cover SOL
}

// ============================================================================
// Natural unlock — production-path tests for `MIN_TREASURY_SOL_FOR_LENDING`
// ============================================================================
//
// Two angles:
//
//   1. `treasury_grows_via_swap_fees_to_sol_path` — build-agnostic mechanism
//      test. Verifies the swap_fees_to_sol instruction converts
//      treasury_token_account balance into treasury.sol_balance. This is
//      the LAST step of the unlock pipeline (`transfer fees accumulate →
//      harvest_fees → swap_fees_to_sol → SOL in treasury`). Bonding fees
//      also flow directly into treasury.sol_balance via protocol_fee.
//
//   2. `devnet_lending_unlocks_naturally_after_bonding` — devnet-only.
//      Proves the shortest unlock path: launch → bond_to_completion →
//      lending available. On devnet (1 SOL gate), the natural protocol
//      fees from bonding completion are enough to clear the gate without
//      any test pokes. Mainnet's 100 SOL gate requires sustained post-
//      launch trading volume to clear naturally — verified in production,
//      not here.

#[test]
fn treasury_grows_via_swap_fees_to_sol_path() {
    // Mechanism test: the production accrual path is
    //   transfer fees (Token-2022 0.07% per transfer) → withholding
    //   → harvest_fees → treasury_token_account → swap_fees_to_sol
    //   → treasury.sol_balance ↑
    //
    // We verify the LAST link: tokens staged in treasury_token_account
    // (representing what sustained volume would accumulate) get
    // converted to SOL when swap_fees_to_sol runs.
    let (mut env, t, _) = migrated();

    // swap_fees_to_sol requires pool > 1.2x baseline (price-up ratio gate)
    let baseline = env.get_treasury(&t).baseline_sol_reserves;
    env.poke_pool_sol(&t, baseline * 2);

    // Stage tokens. 100M raw represents what real volume produces over
    // sustained trading periods (Token-2022 transfer fees compound).
    env.poke_token_amount(t.treasury_token_account, 100_000_000_000_000);

    let before = env.get_treasury(&t).sol_balance;
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.swap_fees_to_sol(&payer, &t, 1)
        .expect("swap_fees_to_sol must succeed with staged tokens + price-ratio gate cleared");
    let after = env.get_treasury(&t).sol_balance;

    assert!(
        after > before,
        "swap_fees_to_sol should grow treasury.sol_balance (before={}, after={})",
        before,
        after,
    );
}

#[test]
#[cfg(feature = "devnet")]
fn devnet_lending_unlocks_naturally_after_bonding() {
    // On devnet (gate = 1 SOL), the natural protocol fees from
    // bond_to_completion are enough to clear the gate. Test the shortest
    // possible unlock path: launch → bond → migrate → lending available.
    // No state pokes, no harvest+swap — just the protocol's own
    // accrual from sustained bonding-curve activity.
    let (mut env, t, borrower) = migrated();

    let tr = env.get_treasury(&t);
    assert!(
        tr.sol_balance >= MIN_TREASURY_SOL_FOR_LENDING,
        "devnet gate should clear from bond fees alone — got sol_balance={}, gate={}",
        tr.sol_balance,
        MIN_TREASURY_SOL_FOR_LENDING,
    );

    // Borrow against natural state — no pokes anywhere.
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.borrow(&borrower, &t, bal / 4, 100_000_000)
        .expect("borrow should succeed on devnet without any state manipulation");
}
