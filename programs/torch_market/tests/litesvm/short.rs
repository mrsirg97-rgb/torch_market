// Short selling tests. The shorter posts SOL collateral, borrows tokens from
// treasury_lock, and pays back the same token amount (typically after the
// token price falls) plus interest.
//
// 20 tests covering open / close / liquidate happy paths and reachable errors.
//
// Setup notes (verified at runtime):
//   pool_sol = 100 SOL, pool_tokens ≈ 149.78M (raw) post-FLAME migration.
//   treasury_lock holds 300M tokens (raw, after Token-2022 fee on initial mint).
//   At default lending_utilization_cap=80%: max_lendable_tokens ≈ 240M raw.
//   treasury.sol_balance ≈ 10.8 SOL post-bonding.
//
// Token-2022 transfer fee (7 bps) gotcha: when treasury_lock sends X tokens to
// the shorter, the shorter receives X*(1-fee_bps). To "fully close" they must
// send back X (face value) — so a fresh shorter is always slightly short of
// closing fully. Tests using a `first_buyer` shorter exploit their existing
// token balance as a top-up.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signature::Keypair, signer::Signer};

use crate::{
    expect_anchor_err, expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError, state::PositionSide};

// `position.tokens_borrowed` records the GROSS amount the lock sent at open
// (v20-current lock-conservation design). The shorter received `gross − fee`
// net but owes the full gross back on close; the close-side gross-up makes
// the cycle net-positive for the lock. See handlers/short.rs::open_short
// comment for the full rationale.

fn migrated() -> (Env, TokenCtx, Keypair) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let first_buyer = env.bond_to_completion(&t);
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");
    env.airdrop(&first_buyer.pubkey(), 5 * LAMPORTS_PER_SOL);
    (env, t, first_buyer)
}

// ============================================================================
// open_short (5: 4 active + 1 ignored vault stub)
// ============================================================================

#[test]
fn open_short_happy() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    // V21: collateral SOL in (index 0, min_out=1); the handler SIZES the token
    // borrow (collateral × LTV, clamped to lock + wallet caps).
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1)
        .expect("open_short");

    let pos = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("short pos");
    assert_eq!(pos.side, PositionSide::Short);
    // collateral_amount is the post-open-fee net (≤ the 1 SOL posted).
    assert!(pos.collateral_amount > 0 && pos.collateral_amount <= LAMPORTS_PER_SOL);
    // Debt is sized by the handler, above the dust floor.
    assert!(pos.debt_amount >= MIN_SHORT_TOKENS, "sized above dust floor");
    // Collateral lives in the per-position SOL vault now, NOT in the treasury.
    let vault = short_vault_pda(&t, &shorter.pubkey(), 0);
    let vault_lamports = env.svm.get_account(&vault).map(|a| a.lamports).unwrap_or(0);
    assert!(vault_lamports > 0, "collateral + sale proceeds in position vault");
}

#[test]
fn open_short_short_not_enabled() {
    let (mut env, t, _) = migrated();
    let mut tr = env.get_treasury(&t);
    tr.short_selling_enabled = false;
    env.poke_anchor(t.treasury, tr);

    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1),
        TorchMarketError::ShortNotEnabled
    );
}

#[test]
fn open_short_not_migrated() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    // Don't migrate.
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1),
        TorchMarketError::NotMigrated
    );
}

#[test]
fn open_short_too_small() {
    // V21: no user token amount — the borrow is sized from collateral. Tiny
    // collateral → sized borrow falls below MIN_SHORT_TOKENS → ShortTooSmall.
    // TUNE: this collateral must be small enough that collateral×LTV (priced
    // into tokens) < MIN_SHORT_TOKENS at the migrated pool ratio.
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, 0, 10_000, 1), // 10k lamports collateral
        TorchMarketError::ShortTooSmall
    );
}

#[test]
fn open_short_clamps_to_caps() {
    // Folds the old ltv/cap/user-cap REJECT tests. V21 clamps instead of
    // rejecting: an oversized collateral implies a borrow above the wallet cap
    // (MAX_WALLET_TOKENS), so the handler opens the position AT the clamped size
    // rather than erroring. The UI shows the clamped size pre-sign; min_out
    // guards the swap. Assert: position opens, debt == the cap (not larger).
    let (mut env, t, _) = migrated();

    // Stage the lock with plenty so the lock-balance clamp doesn't bind first;
    // the wallet cap (MAX_WALLET_TOKENS) is the intended binding clamp.
    env.poke_token_amount(t.treasury_lock_token_account, 100_000_000_000_000_000);
    // Deep pool: enough SOL depth that a large collateral prices to a borrow
    // well above MAX_WALLET_TOKENS.
    env.poke_pool_sol(&t, 1000 * LAMPORTS_PER_SOL);
    env.poke_token_amount(t.deep_pool_token_vault, 100_000_000_000_000);

    // TUNE: collateral large enough that collateral×LTV in tokens > MAX_WALLET_TOKENS.
    let shorter = env.new_funded(1500 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, 1000 * LAMPORTS_PER_SOL, 1)
        .expect("open_short clamps, does not reject");

    let pos = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("short pos");
    assert_eq!(
        pos.debt_amount, MAX_WALLET_TOKENS,
        "borrow clamped to the per-wallet cap"
    );
}

#[test]
fn open_short_via_vault_happy() {
    // A linked wallet opens a short funded by the VAULT. The position is
    // vault-seeded (owned by the vault); the collateral comes from vault_sol; the
    // signer only pays rent — it never touches the vault funds beyond the trade.
    let (mut env, t, _liquidator) = migrated();
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner); // owner is the auto-linked wallet
    env.deposit_vault(&owner, &vault, 3 * LAMPORTS_PER_SOL)
        .expect("deposit");

    let vault_sol_before = env.vault_sol(&vault);
    env.open_short_via_vault(&owner, &vault, &t, 0, LAMPORTS_PER_SOL, 1)
        .expect("open_short_via_vault");

    // Position is VAULT-SEEDED and owned by the vault.
    let pos = env
        .get_position(&t, &vault.vault, POSITION_SIDE_SHORT, 0)
        .expect("vault-seeded position exists");
    assert!(pos.debt_amount > 0, "borrowed tokens against the vault");
    assert_eq!(pos.user, vault.vault, "position owned by the vault, not a wallet");

    // The vault funded the collateral (vault_sol dropped by exactly `collateral`);
    // the SOL is now in the per-position vault, not back in the linked wallet.
    assert_eq!(
        env.vault_sol(&vault),
        vault_sol_before - LAMPORTS_PER_SOL,
        "collateral came from the vault"
    );
    let pos_vault = short_vault_pda(&t, &vault.vault, 0);
    let pos_vault_sol = env.svm.get_account(&pos_vault).map(|a| a.lamports).unwrap_or(0);
    assert!(
        pos_vault_sol > 0,
        "per-position vault holds net collateral + sale proceeds"
    );
}

// ============================================================================
// close_short (6)
// ============================================================================

#[test]
fn close_short_partial() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let before = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");

    // V21: partial close by FRACTION. Vault SOL buys back half the debt; the
    // shorter no longer needs a token balance (the vault funds the buy).
    env.close_short(&shorter, &t, 0, 5_000, 0)
        .expect("partial close");

    let after = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos still open");
    // Debt reduced ~half (allow ±1 for fractional rounding); position stays open.
    let repaid = before.debt_amount - after.debt_amount;
    assert!(
        repaid >= before.debt_amount / 2 - 1 && repaid <= before.debt_amount / 2 + 1,
        "≈half the debt repaid (before={}, after={})",
        before.debt_amount,
        after.debt_amount
    );
    assert!(after.debt_amount > 0, "still open after partial");
}

#[test]
fn close_short_full() {
    // Full close: vault SOL buys back the whole debt, leftover surplus → user.
    // (PDA-close itself is covered in tier_b; here we assert the surplus payout.)
    let (mut env, t, shorter) = migrated();
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");

    let before_lamports = env.svm.get_account(&shorter.pubkey()).unwrap().lamports;
    env.close_short(&shorter, &t, 0, 10_000, 0).expect("full close");

    assert!(
        env.get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
            .is_none(),
        "position closed after full close"
    );
    let after_lamports = env.svm.get_account(&shorter.pubkey()).unwrap().lamports;
    // Surplus vault SOL + reclaimed rent flow back to the shorter on full close.
    assert!(
        after_lamports > before_lamports,
        "surplus + rent returned to user (before={}, after={})",
        before_lamports,
        after_lamports
    );
}

#[test]
fn close_short_interest_first() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let opened = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");

    // Warp to accrue token-denominated interest, then close a tiny fraction.
    // Interest is credited before principal, so a small fractional close eats
    // into accrued_interest while leaving the principal (debt_amount) intact.
    // 150 bps/epoch over ~1.5M slots ≈ 1.49% interest, comfortably above 1%.
    env.warp_to_slot(env.current_slot() + 1_500_000);
    // TUNE: 1% fraction should be ≤ accrued interest at this warp; if principal
    // moves, lower the fraction or lengthen the warp.
    env.close_short(&shorter, &t, 0, 100, 0)
        .expect("tiny interest-first close");

    let pos = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");
    assert_eq!(
        pos.debt_amount, opened.debt_amount,
        "principal untouched (interest paid first)"
    );
}

#[test]
fn close_short_no_active_short() {
    // V21 can't open a debt-0 short (handler floors at MIN_SHORT_TOKENS), and a
    // full close removes the Position PDA — so the handler's `NoActiveShort`
    // guard is unreachable as a custom error. Re-closing a closed position fails
    // at account resolution with Anchor's AccountNotInitialized (the PDA is gone).
    let (mut env, t, shorter) = migrated();
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    env.close_short(&shorter, &t, 0, 10_000, 0).expect("full close");

    expect_anchor_err!(
        env.close_short(&shorter, &t, 0, 10_000, 0),
        crate::harness::ANCHOR_ACCOUNT_NOT_INITIALIZED
    );
}

#[test]
fn close_short_zero_amount() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    // repay_fraction_bps = 0 is rejected by the context constraint.
    expect_err!(
        env.close_short(&shorter, &t, 0, 0, 0),
        TorchMarketError::ZeroAmount
    );
}

#[test]
fn close_short_via_vault_happy() {
    // Open + full-close a vault-owned short. Surplus P&L must return to the VAULT
    // (vault_sol), NOT the linked wallet that signed the close — the signer only
    // gets its own position rent back.
    let (mut env, t, _liq) = migrated();
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);
    env.deposit_vault(&owner, &vault, 3 * LAMPORTS_PER_SOL)
        .expect("deposit");
    env.open_short_via_vault(&owner, &vault, &t, 0, LAMPORTS_PER_SOL, 1)
        .expect("open");

    let vault_sol_after_open = env.vault_sol(&vault);

    env.close_short_via_vault(&owner, &vault, &t, 0, 10_000, 0)
        .expect("close_short_via_vault");

    // Position is gone (vault-seeded).
    assert!(
        env.get_position(&t, &vault.vault, POSITION_SIDE_SHORT, 0)
            .is_none(),
        "position closed"
    );
    // Surplus (most of the collateral on an immediate close) returned to the vault.
    assert!(
        env.vault_sol(&vault) > vault_sol_after_open,
        "surplus returned to vault_sol, not the signer wallet"
    );
}

// ============================================================================
// liquidate_short (6)
// ============================================================================

#[test]
fn liquidate_short_happy() {
    // Open a short, pump pool_sol so the token debt is worth more SOL (LTV
    // breaches 65%), warm the TWAP AFTER the pump so both the spot and TWAP
    // marks breach, then liquidate. Liquidator covers up to 50% of the debt.
    let (mut env, t, liquidator) = migrated(); // first_buyer has tokens to cover
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let before = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");

    // TUNE: pump to cross the 65% liq threshold at the TWAP mark (≈3× spot →
    // ~77% LTV for this short; the keeperless mark = the held/poked price).
    env.poke_pool_sol(&t, 300 * LAMPORTS_PER_SOL);
    env.warm_twap(&cranker, &t); // warms at the pumped price → twap_ltv breaches

    env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0)
        .expect("liquidate_short");

    let after = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");
    assert!(after.debt_amount < before.debt_amount, "debt reduced");
    // Collateral is in the per-position SOL vault; seizure drains it.
    let vault = short_vault_pda(&t, &shorter.pubkey(), 0);
    let vault_lamports = env.svm.get_account(&vault).map(|a| a.lamports).unwrap_or(0);
    assert!(vault_lamports < LAMPORTS_PER_SOL, "vault SOL seized");
}

#[test]
fn liquidate_short_not_liquidatable() {
    // Healthy short, TWAP warmed at the healthy price → trigger refuses.
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    env.warm_twap(&cranker, &t); // warm at healthy price; LTV well below 65%

    expect_err!(
        env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0),
        TorchMarketError::ShortNotLiquidatable
    );
}

#[test]
fn liquidate_short_partial_close_bps() {
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let before = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .unwrap();

    env.poke_pool_sol(&t, 300 * LAMPORTS_PER_SOL); // TUNE: ~77% LTV at the mark
    env.warm_twap(&cranker, &t);
    env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0)
        .expect("liquidate");

    let after = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .unwrap();
    let covered = before.debt_amount - after.debt_amount;
    // DEFAULT_LIQUIDATION_CLOSE_BPS = 50%: at most half the debt per call.
    assert!(covered <= before.debt_amount / 2 + 1, "≤50% covered per call");
    assert!(after.debt_amount > 0, "still has debt after partial");
}

#[test]
fn liquidate_short_bad_debt() {
    // Pump pool_sol so high the debt value far exceeds the vault collateral: the
    // seize is vault-capped (insolvent), so the position is FULLY resolved in one
    // liquidation — entire residual debt written off, position + vault closed,
    // total_tokens_lent returns to truth. (Pre-fix this left an un-liquidatable tail.)
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let lent_before = env.get_treasury(&t).total_tokens_lent;
    assert!(lent_before > 0);

    // TUNE: large pump so target seize > vault → insolvent / bad debt path.
    env.poke_pool_sol(&t, 1000 * LAMPORTS_PER_SOL);
    env.warm_twap(&cranker, &t);
    env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0)
        .expect("liquidate");

    // Position fully resolved: PDA closed (no stuck tail), vault drained + reaped.
    assert!(
        env.get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
            .is_none(),
        "insolvent short fully liquidated + closed"
    );
    let vault = short_vault_pda(&t, &shorter.pubkey(), 0);
    let vault_lamports = env.svm.get_account(&vault).map(|a| a.lamports).unwrap_or(0);
    assert_eq!(vault_lamports, 0, "all vault SOL seized");
    // Residual debt written off the global counter.
    let tr = env.get_treasury(&t);
    assert_eq!(tr.active_shorts, 0, "active_shorts decremented");
    assert!(
        tr.total_tokens_lent < lent_before,
        "residual token debt written off the lent counter"
    );
}

#[test]
fn liquidate_short_warmup_fail_closed() {
    // D-10: with NO warm TWAP (ring < 2 observations), the liquidation trigger
    // has no hardened mark to read and MUST fail closed — even when the position
    // is genuinely underwater at spot. This is the manipulation guard's core:
    // an atomic spot pump can't manufacture a liquidation without a warm ring.
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");

    // Pump spot underwater but DO NOT warm the ring.
    env.poke_pool_sol(&t, 200 * LAMPORTS_PER_SOL);

    expect_err!(
        env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0),
        TorchMarketError::ShortNotLiquidatable
    );
}

#[test]
fn liquidate_short_spot_veto_when_recovered() {
    // Asymmetric spot veto (MEDIUM-2 fix): IDENTICAL setup to liquidate_short_happy
    // (warm the TWAP at the pumped price → twap_ltv breaches), but then spot RECOVERS
    // to clearly healthy before the liquidation. The spot veto must refuse — a
    // genuinely-recovered borrower isn't liquidated on the stale-high TWAP. The only
    // difference vs the happy test is the recovery poke, so the refusal is the spot
    // gate (the TWAP mark is unchanged: liquidate runs in the same slot, the
    // current-spot extrapolation over the ~0 gap is negligible).
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");

    env.poke_pool_sol(&t, 300 * LAMPORTS_PER_SOL); // underwater at the mark
    env.warm_twap(&cranker, &t); // TWAP marks the pumped price (twap_ltv > 65%)
    env.poke_pool_sol(&t, 50 * LAMPORTS_PER_SOL); // spot recovers CLEARLY healthy (< floor)

    expect_err!(
        env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0),
        TorchMarketError::ShortNotLiquidatable
    );
}

#[test]
fn liquidate_short_proceeds_when_spot_in_gray_zone() {
    // Dodge-resistance (MEDIUM-2 fix): TWAP breaches, and spot is only MILDLY healthy
    // (in the gray zone between the veto floor and the liq threshold — i.e. spot was
    // nudged just under the threshold, as a dodging borrower would). The asymmetric
    // veto requires spot to be CLEARLY healthy (< floor) to refuse, so the
    // liquidation PROCEEDS. (Pre-fix, `spot_ltv > threshold` would have refused here,
    // which is exactly the cheap atomic dodge we closed.)
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1).expect("open");
    let before = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");

    env.poke_pool_sol(&t, 300 * LAMPORTS_PER_SOL); // underwater at the mark
    env.warm_twap(&cranker, &t); // TWAP marks the pumped price
    // Spot nudged into the gray zone (veto-floor 5500 < ltv < threshold 6500). The
    // [V21] depth curve opens this floor-depth (~100 SOL) pool's short at 30% LTV
    // (LTV_MIN), so the gray band lands at ~260 SOL of poked spot (≈5990 bps), not
    // the old ladder's 35%-LTV calibration. See docs/depth-scaled-risk-rails.md.
    env.poke_pool_sol(&t, 260 * LAMPORTS_PER_SOL);

    env.liquidate_short(&liquidator, shorter.pubkey(), &t, 0)
        .expect("liquidates despite gray-zone spot (no clear-recovery veto)");
    let after = env
        .get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
        .expect("pos");
    assert!(after.debt_amount < before.debt_amount, "debt reduced");
}

#[test]
fn liquidate_short_via_vault_happy() {
    // Open a VAULT-OWNED short, pump pool_sol so the token debt breaches the 65%
    // LTV at the TWAP mark, then liquidate as an EXTERNAL actor (no link). Seized
    // SOL → liquidator; the position's vault SOL is drained by the seizure.
    let (mut env, t, liquidator) = migrated();
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);
    env.deposit_vault(&owner, &vault, 3 * LAMPORTS_PER_SOL)
        .expect("deposit");
    env.open_short_via_vault(&owner, &vault, &t, 0, LAMPORTS_PER_SOL, 1)
        .expect("open");
    let before = env
        .get_position(&t, &vault.vault, POSITION_SIDE_SHORT, 0)
        .expect("pos");

    // TUNE: pump to cross the 65% liq threshold at the TWAP mark (mirror of the
    // wallet-custodied liquidate_short_happy tuning).
    env.poke_pool_sol(&t, 300 * LAMPORTS_PER_SOL);
    env.warm_twap(&cranker, &t);

    let liq_before = env.svm.get_account(&liquidator.pubkey()).unwrap().lamports;
    env.liquidate_short_via_vault(&liquidator, &vault, &t, 0)
        .expect("liquidate_short_via_vault");

    let after = env
        .get_position(&t, &vault.vault, POSITION_SIDE_SHORT, 0)
        .expect("pos");
    assert!(after.debt_amount < before.debt_amount, "debt reduced");
    // Seizure drains the per-position vault SOL (keyed by the vault).
    let pos_vault = short_vault_pda(&t, &vault.vault, 0);
    let pos_vault_lamports = env.svm.get_account(&pos_vault).map(|a| a.lamports).unwrap_or(0);
    assert!(pos_vault_lamports < LAMPORTS_PER_SOL, "position vault SOL seized");
    // Seized SOL paid out to the external liquidator (net of tx fee it covered).
    let liq_after = env.svm.get_account(&liquidator.pubkey()).unwrap().lamports;
    assert!(liq_after > liq_before, "liquidator received seized SOL");
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn short_vault_pda(
    t: &TokenCtx,
    user: &solana_sdk::pubkey::Pubkey,
    index: u32,
) -> solana_sdk::pubkey::Pubkey {
    solana_sdk::pubkey::Pubkey::find_program_address(
        &[
            SHORT_VAULT_SEED,
            user.as_ref(),
            t.mint.as_ref(),
            &index.to_le_bytes(),
        ],
        &torch_market::ID,
    )
    .0
}
