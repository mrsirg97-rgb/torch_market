// Lending tests (long): borrow / repay / liquidate.
//
// 22 tests covering every reachable variant in the lending path.
// Setup notes:
//   - FLAME bonding gives pool ~100 SOL / 150M tokens post-migration.
//   - First buyer holds ~19M tokens (just under wallet cap) — ideal borrower.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signature::Keypair, signer::Signer};

use crate::{
    expect_anchor_err, expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError, state::PositionSide};

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
    // V21: treasury SOL is the System-owned treasury_sol_vault's lamports (derived,
    // no tracked field) — fund it directly to clear the lending gate.
    env.airdrop(&t.treasury_sol_vault, 200 * LAMPORTS_PER_SOL);
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
// open_long (9: 8 active + 1 ignored vault stub)
// ============================================================================

#[test]
fn open_long_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    // V21: deposit token collateral (index 0, min_out=1). The handler sizes the
    // SOL borrow (collateral × LTV, clamped) and atomically buys tokens into the
    // position vault. No user borrow amount.
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open_long");

    let pos = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("position exists");
    assert_eq!(pos.side, PositionSide::Long);
    assert!(pos.debt_amount > 0, "borrowed SOL (gross debt)");
    assert!(pos.collateral_amount > 0, "token collateral recorded");
    let tr = env.get_treasury(&t);
    assert_eq!(tr.total_sol_lent_to_longs, pos.debt_amount);
    assert_eq!(tr.active_longs, 1);
}

#[test]
fn open_long_lending_not_enabled() {
    let (mut env, t, borrower) = lending_ready();
    let mut tr = env.get_treasury(&t);
    tr.lending_enabled = false;
    env.poke_anchor(t.treasury, tr);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.open_long(&borrower, &t, 0, bal / 2, 1),
        TorchMarketError::LendingNotEnabled
    );
}

#[test]
fn open_long_requires_migration() {
    // Not migrated → the bonding-curve constraint rejects.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed");

    let bal = token_balance(&env, &buyer.pubkey(), &t.mint);
    expect_err!(
        env.open_long(&buyer, &t, 0, bal / 2, 1),
        TorchMarketError::LendingRequiresMigration
    );
}

#[test]
fn open_long_lending_not_yet_unlocked() {
    // Lending floor gate: treasury below the unlock threshold → reject.
    // (Long-only — shorts borrow tokens from the lock, not SOL from treasury,
    // so they don't gate on this.) Build-independent: 0.5 SOL is below the
    // simnet/devnet gate (1) AND the mainnet gate (100), so the lock fires
    // under every build.
    let (mut env, t, borrower) = migrated();
    // 0.5 SOL physical in the vault — below every gate (simnet/devnet 1, mainnet 100).
    env.poke_lamports(&t.treasury_sol_vault, 500_000_000);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.open_long(&borrower, &t, 0, bal, 1),
        TorchMarketError::LendingNotYetUnlocked
    );
}

#[test]
fn open_long_lending_cap_exceeded() {
    // Gate cleared, but the pool's lending capacity is effectively exhausted:
    // poke the utilization cap to 1 bp so max_lendable ≈ 0.02 SOL → global
    // headroom < MIN_BORROW_AMOUNT → LendingCapExceeded (the explicit reject we
    // kept distinct from the dust floor).
    let (mut env, t, borrower) = lending_ready();
    let mut tr = env.get_treasury(&t);
    tr.lending_utilization_cap_bps = 1;
    env.poke_anchor(t.treasury, tr);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.open_long(&borrower, &t, 0, bal / 2, 1),
        TorchMarketError::LendingCapExceeded
    );
}

#[test]
fn open_long_clamps_to_caps() {
    // Folds the old ltv_exceeded / user_cap_exceeded / depth_tier_zero REJECT
    // tests. V21 sizes the borrow at the depth-band LTV and CLAMPS to the
    // per-user absolute cap (20% of lendable) — it never rejects an "oversized"
    // request, because there is no user borrow amount. A large collateral that
    // would imply a borrow above the per-user cap simply opens AT the cap.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);

    // Compute the expected cap from the DERIVED treasury balance BEFORE the borrow
    // (treasury_sol_vault lamports − rent; the airdrop + bonding fees, not a round
    // 200 SOL). Mirror the handler's apply_bps order (multiply then ÷10_000).
    let tr = env.get_treasury(&t);
    let available = env.treasury_sol(&t);
    let max_lendable = available * tr.lending_utilization_cap_bps as u64 / 10_000;
    let absolute_cap = max_lendable * MAX_USER_BORROW_SHARE_BPS as u64 / 10_000;

    // Full token balance as collateral → implied borrow exceeds the per-user cap;
    // handler clamps. Position opens; debt == the absolute cap.
    env.open_long(&borrower, &t, 0, bal, 1)
        .expect("open_long clamps, does not reject");

    let pos = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("position exists");
    assert_eq!(pos.debt_amount, absolute_cap, "borrow clamped to per-user cap");
}

#[test]
fn open_long_below_min_amount() {
    // Tiny collateral → sized borrow falls below MIN_BORROW_AMOUNT → BorrowTooSmall
    // (the dust floor, distinct from the pool-exhausted LendingCapExceeded above).
    let (mut env, t, borrower) = lending_ready();
    // TUNE: collateral small enough that collateral×LTV < MIN_BORROW_AMOUNT.
    expect_err!(
        env.open_long(&borrower, &t, 0, 1_000, 1),
        TorchMarketError::BorrowTooSmall
    );
}

#[test]
fn open_long_pool_too_thin() {
    // Pool < MIN_POOL_SOL_LENDING → depth tier 0 LTV → PoolTooThin. New positions
    // blocked at thin pools (liquidations of existing ones still allowed).
    let (mut env, t, borrower) = migrated();
    env.airdrop(&t.treasury_sol_vault, 200 * LAMPORTS_PER_SOL); // clear the lending floor first
    env.poke_pool_sol(&t, MIN_POOL_SOL_LENDING - 1);

    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    expect_err!(
        env.open_long(&borrower, &t, 0, bal / 2, 1),
        TorchMarketError::PoolTooThin
    );
}

#[test]
fn open_long_via_vault_happy() {
    // VAULT-OWNED long: collateral tokens come from the vault's token ATA (not a
    // wallet), the borrow is from the treasury, and the position is vault-seeded.
    // `owner` (the vault creator) is an auto-linked wallet → valid signer.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);
    env.fund_vault_tokens(&owner, &vault, &t, bal); // stock vault collateral

    env.open_long_via_vault(&owner, &vault, &t, 0, bal / 2, 1)
        .expect("open_long_via_vault");

    let pos = env
        .get_position(&t, &vault.vault, POSITION_SIDE_LONG, 0)
        .expect("vault-owned position exists");
    assert_eq!(pos.side, PositionSide::Long);
    assert_eq!(pos.user, vault.vault, "position owned by the vault");
    assert!(pos.debt_amount > 0, "borrowed SOL (gross debt)");
    assert!(pos.collateral_amount > 0, "token collateral recorded");
    let tr = env.get_treasury(&t);
    assert_eq!(tr.total_sol_lent_to_longs, pos.debt_amount);
    assert_eq!(tr.active_longs, 1);
}

// ============================================================================
// close_long (6: 5 active + 1 ignored vault stub)
// ============================================================================

#[test]
fn close_long_partial() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");
    let before = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos");

    // V21: partial close by FRACTION. Sells half the vault tokens, repays half
    // the SOL debt → treasury; position stays open. 0 slots → no interest.
    env.close_long(&borrower, &t, 0, 5_000, 0).expect("partial close");

    let after = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos still open");
    let repaid = before.debt_amount - after.debt_amount;
    assert!(
        repaid >= before.debt_amount / 2 - 1 && repaid <= before.debt_amount / 2 + 1,
        "≈half the SOL debt repaid"
    );
    assert!(after.debt_amount > 0, "still open after partial");
}

#[test]
fn close_long_full_returns_surplus() {
    // V21 full close SELLS all vault tokens, repays debt → treasury, and returns
    // the SOL SURPLUS (not the original collateral tokens) to the borrower. The
    // position PDA + token vault are closed.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");

    let lamports_before = env.svm.get_account(&borrower.pubkey()).unwrap().lamports;
    env.close_long(&borrower, &t, 0, 10_000, 0).expect("full close");

    assert!(
        env.get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
            .is_none(),
        "position closed on full close"
    );
    let lamports_after = env.svm.get_account(&borrower.pubkey()).unwrap().lamports;
    assert!(
        lamports_after > lamports_before,
        "SOL surplus + reclaimed rent returned to borrower"
    );
    let tr = env.get_treasury(&t);
    assert_eq!(tr.active_longs, 0);
}

#[test]
fn close_long_interest_first() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");
    let opened = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos");

    // Warp to accrue SOL-denominated interest, then close a tiny fraction.
    // Interest credits before principal, so a 1% close leaves debt_amount intact.
    // At 150 bps/epoch over ~1.512M slots/epoch, ~1.5M slots accrues ~1.49%
    // interest, so a 1% close is fully absorbed by interest (principal untouched).
    env.warp_to_slot(env.current_slot() + 1_500_000);
    env.close_long(&borrower, &t, 0, 100, 0)
        .expect("tiny interest-first close");

    let pos = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos");
    assert_eq!(
        pos.debt_amount, opened.debt_amount,
        "principal untouched (interest paid first)"
    );
}

#[test]
fn close_long_no_active_loan() {
    // V21 can't open a debt-0 long, and a full close removes the Position PDA —
    // so the handler's `NoActiveLoan` guard is unreachable as a custom error.
    // Re-closing a closed position fails at account resolution with Anchor's
    // AccountNotInitialized (the PDA is gone).
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");
    env.close_long(&borrower, &t, 0, 10_000, 0).expect("full close");

    expect_anchor_err!(
        env.close_long(&borrower, &t, 0, 10_000, 0),
        crate::harness::ANCHOR_ACCOUNT_NOT_INITIALIZED
    );
}

#[test]
fn close_long_zero_amount() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");
    // repay_fraction_bps = 0 → context constraint rejects.
    expect_err!(
        env.close_long(&borrower, &t, 0, 0, 0),
        TorchMarketError::ZeroAmount
    );
}

#[test]
fn close_long_via_vault_happy() {
    // Open + full-close a vault-owned long. The SOL surplus must return to the
    // VAULT (vault_sol), NOT the linked wallet that signed the close — the signer
    // only reclaims its own position + token-vault rent.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);
    env.fund_vault_tokens(&owner, &vault, &t, bal);
    env.open_long_via_vault(&owner, &vault, &t, 0, bal / 2, 1)
        .expect("open");

    let vault_sol_before = env.vault_sol(&vault);
    env.close_long_via_vault(&owner, &vault, &t, 0, 10_000, 0)
        .expect("close_long_via_vault");

    assert!(
        env.get_position(&t, &vault.vault, POSITION_SIDE_LONG, 0)
            .is_none(),
        "position closed on full close"
    );
    assert!(
        env.vault_sol(&vault) > vault_sol_before,
        "SOL surplus returned to vault_sol, not the signer wallet"
    );
    let tr = env.get_treasury(&t);
    assert_eq!(tr.active_longs, 0);
}

// ============================================================================
// liquidate_long (6: 5 active + 1 ignored vault stub)
// ============================================================================

#[test]
fn liquidate_long_happy() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    // Open a long, drop the token price (pool_sol down) so the vault tokens are
    // worth less than the SOL debt (LTV breaches 65%), warm the TWAP AFTER the
    // drop so the mark breaches too, then liquidate.
    env.open_long(&borrower, &t, 0, bal, 1).expect("open");
    let before = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos");

    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    // TUNE: moderate drop → underwater but SOLVENT (vault still covers the 50% slice
    // + bonus) so this is a partial liquidation, position stays open.
    env.poke_pool_sol(&t, 120 * LAMPORTS_PER_SOL);
    env.warm_twap(&cranker, &t);

    let liquidator = env.new_funded(25 * LAMPORTS_PER_SOL);
    env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0)
        .expect("liquidate");

    let after = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .expect("pos");
    assert!(after.debt_amount < before.debt_amount, "debt reduced");
    assert!(after.debt_amount > 0, "partial: position still open");
}

#[test]
fn liquidate_long_not_liquidatable() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open");

    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.warm_twap(&cranker, &t); // warm at healthy price; LTV well below 65%

    let liquidator = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0),
        TorchMarketError::NotLiquidatable
    );
}

#[test]
fn liquidate_long_partial_capped_at_close_bps() {
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal, 1).expect("open");
    let before = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .unwrap();

    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.poke_pool_sol(&t, 120 * LAMPORTS_PER_SOL); // TUNE: underwater but solvent → partial
    env.warm_twap(&cranker, &t);

    let liquidator = env.new_funded(25 * LAMPORTS_PER_SOL);
    env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0)
        .expect("liquidate");

    let after = env
        .get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
        .unwrap();
    let covered = before.debt_amount - after.debt_amount;
    // DEFAULT_LIQUIDATION_CLOSE_BPS = 50%: at most half the debt per call.
    assert!(covered <= before.debt_amount / 2 + 1_000_000, "≤50% per call");
    assert!(after.debt_amount > 0, "still has debt after partial");
}

#[test]
fn liquidate_long_full_with_bad_debt() {
    // Crush the token price so the vault is worth far less than the debt: the seize
    // is collateral-capped (insolvent), so the position is FULLY resolved in one
    // liquidation — the entire residual debt is written off, the position + token
    // vault are closed, and total_sol_lent_to_longs drops to reflect the loss.
    // (Pre-fix this left an un-liquidatable debt tail + inflated the counter forever.)
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal, 1).expect("open");
    let lent_before = env.get_treasury(&t).total_sol_lent_to_longs;
    assert!(lent_before > 0);

    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.poke_pool_sol(&t, 6 * LAMPORTS_PER_SOL); // TUNE: deep crush → insolvent
    env.warm_twap(&cranker, &t);

    let liquidator = env.new_funded(25 * LAMPORTS_PER_SOL);
    env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0)
        .expect("liquidate");

    // Position fully resolved: PDA closed, no un-liquidatable tail.
    assert!(
        env.get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
            .is_none(),
        "insolvent position fully liquidated + closed (no stuck tail)"
    );
    // Token vault closed (account gone).
    let vault = get_associated_token_address_2022_local(
        &long_position_pda(&t, &borrower.pubkey(), 0),
        &t.mint,
    );
    assert!(env.svm.get_account(&vault).is_none(), "token vault closed");
    // The full debt was written off the global counter (returns to truth).
    let tr = env.get_treasury(&t);
    assert_eq!(tr.active_longs, 0, "active_longs decremented");
    assert!(
        tr.total_sol_lent_to_longs < lent_before,
        "residual debt written off the lent counter"
    );
}

#[test]
fn liquidate_long_warmup_fail_closed() {
    // D-10: no warm TWAP (ring < 2 obs) → liquidation fails closed even though
    // the position is underwater at spot. The manipulation guard: an atomic spot
    // crush can't manufacture a liquidation without a warm ring.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal, 1).expect("open");

    env.poke_pool_sol(&t, 7 * LAMPORTS_PER_SOL); // underwater at spot, ring NOT warmed

    let liquidator = env.new_funded(25 * LAMPORTS_PER_SOL);
    expect_err!(
        env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0),
        TorchMarketError::NotLiquidatable
    );
}

#[test]
fn liquidate_long_via_vault_happy() {
    // Open a VAULT-OWNED long, drop the token price so LTV breaches but the position
    // stays SOLVENT (partial liquidation), then liquidate as an EXTERNAL actor.
    // Liquidator pays SOL debt → seizes vault tokens; position stays open.
    let (mut env, t, borrower) = lending_ready();
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);
    env.fund_vault_tokens(&owner, &vault, &t, bal);
    env.open_long_via_vault(&owner, &vault, &t, 0, bal, 1).expect("open");
    let before = env
        .get_position(&t, &vault.vault, POSITION_SIDE_LONG, 0)
        .expect("pos");

    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.poke_pool_sol(&t, 120 * LAMPORTS_PER_SOL); // TUNE: underwater but solvent → partial
    env.warm_twap(&cranker, &t);

    let liquidator = env.new_funded(25 * LAMPORTS_PER_SOL);
    env.liquidate_long_via_vault(&liquidator, &vault, &t, 0)
        .expect("liquidate_long_via_vault");

    let after = env
        .get_position(&t, &vault.vault, POSITION_SIDE_LONG, 0)
        .expect("pos");
    assert!(after.debt_amount < before.debt_amount, "debt reduced");
    assert!(after.debt_amount > 0, "partial: position still open");
    // Liquidator received seized tokens.
    let seized = token_balance(&env, &liquidator.pubkey(), &t.mint);
    assert!(seized > 0, "liquidator received seized vault tokens");
}

// ---------------------------------------------------------------------------
// helpers (long position / vault PDA derivation)
// ---------------------------------------------------------------------------

fn long_position_pda(
    t: &TokenCtx,
    user: &solana_sdk::pubkey::Pubkey,
    index: u32,
) -> solana_sdk::pubkey::Pubkey {
    solana_sdk::pubkey::Pubkey::find_program_address(
        &[
            POSITION_SEED,
            user.as_ref(),
            t.mint.as_ref(),
            &[POSITION_SIDE_LONG],
            &index.to_le_bytes(),
        ],
        &torch_market::ID,
    )
    .0
}

fn get_associated_token_address_2022_local(
    owner: &solana_sdk::pubkey::Pubkey,
    mint: &solana_sdk::pubkey::Pubkey,
) -> solana_sdk::pubkey::Pubkey {
    torch_market::token_2022_utils::get_associated_token_address_2022(owner, mint)
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

    let before = env.treasury_sol(&t);
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.swap_fees_to_sol(&payer, &t, 1)
        .expect("swap_fees_to_sol must succeed with staged tokens + price-ratio gate cleared");
    let after = env.treasury_sol(&t);

    assert!(
        after > before,
        "swap_fees_to_sol should grow the treasury_sol_vault balance (before={}, after={})",
        before,
        after,
    );
}

#[test]
#[cfg(any(feature = "devnet", feature = "simnet"))]
fn lending_unlocks_naturally_after_bonding() {
    // On simnet/devnet (gate = 1 SOL), the natural protocol fees from
    // bond_to_completion are enough to clear the gate. Test the shortest
    // possible unlock path: launch → bond → migrate → lending available.
    // No state pokes, no harvest+swap — just the protocol's own
    // accrual from sustained bonding-curve activity. Excluded on mainnet
    // (100 SOL gate needs sustained post-launch volume — verified in prod).
    let (mut env, t, borrower) = migrated();

    assert!(
        env.treasury_sol(&t) >= MIN_TREASURY_SOL_FOR_LENDING,
        "1 SOL gate should clear from bond fees alone — got treasury_sol={}, gate={}",
        env.treasury_sol(&t),
        MIN_TREASURY_SOL_FOR_LENDING,
    );

    // Open a long against natural state — no pokes anywhere.
    let bal = token_balance(&env, &borrower.pubkey(), &t.mint);
    env.open_long(&borrower, &t, 0, bal / 4, 1)
        .expect("open_long should succeed on devnet without any state manipulation");
}
