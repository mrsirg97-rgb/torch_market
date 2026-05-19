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
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{
    constants::*, errors::TorchMarketError, token_2022_utils::TOKEN_2022_PROGRAM_ID,
};

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
// open_short (8)
// ============================================================================

#[test]
fn open_short_happy() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open_short");

    let pos = env.get_short(&t, &shorter.pubkey()).expect("short pos");
    assert_eq!(pos.sol_collateral, LAMPORTS_PER_SOL);
    assert_eq!(pos.tokens_borrowed, MIN_SHORT_TOKENS);
    let tr = env.get_treasury(&t);
    // Main encodes short collateral in total_burned_from_buyback (repurposed
    // field; see state.rs:122). Sentinel-style storage so layout stays stable.
    assert_eq!(tr.total_burned_from_buyback, LAMPORTS_PER_SOL);
}

#[test]
fn open_short_short_not_enabled() {
    let (mut env, t, _) = migrated();
    let mut tr = env.get_treasury(&t);
    // Main encodes "short selling enabled" as buyback_percent_bps == u16::MAX
    // sentinel. Flip to 0 to disable.
    tr.buyback_percent_bps = 0;
    env.poke_anchor(t.treasury, tr);

    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS),
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
        env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS),
        TorchMarketError::NotMigrated
    );
}

#[test]
fn open_short_too_small() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS - 1),
        TorchMarketError::ShortTooSmall
    );
}

#[test]
fn open_short_ltv_exceeded() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(LAMPORTS_PER_SOL);
    // 1 SOL collateral. At 35% LTV cap, max debt_value = 0.35 SOL. That maps
    // to ~524k display tokens (5.24e11 raw). Borrowing 1B tokens raw (= ~668 SOL
    // debt_value) blows the LTV cap by 4 orders of magnitude.
    let huge_borrow = 1_000_000_000_000_000; // 1B display tokens raw
    expect_err!(
        env.open_short(&shorter, &t, LAMPORTS_PER_SOL / 10, huge_borrow),
        TorchMarketError::LtvExceeded
    );
}

#[test]
fn open_short_cap_exceeded() {
    let (mut env, t, _) = migrated();
    // Set utilization cap to 0% → max_lendable_tokens = 0. Any short fails ShortCap.
    let mut tr = env.get_treasury(&t);
    tr.lending_utilization_cap_bps = 0;
    env.poke_anchor(t.treasury, tr);

    let shorter = env.new_funded(2 * LAMPORTS_PER_SOL); // 2 SOL — covers 1 SOL collateral + tx fee
    expect_err!(
        env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS),
        TorchMarketError::ShortCapExceeded
    );
}

#[test]
fn open_short_user_cap_exceeded() {
    // Trigger UserShortCap without LendingCap firing first.
    // Need: tokens_to_borrow > user_cap AND tokens_to_borrow <= short_cap AND LTV passes.
    //
    // utilization_cap = 1 bp (0.01%) → max_lendable_tokens = 300M raw * 1/10000 = 3e10 raw.
    // Collateral 2M lamports (0.002 SOL — just above LTV min for MIN_SHORT_TOKENS).
    //   LTV: 1e9 raw debt → 668k lamports debt_value. 668k/2M = 33.4% < 35% cap ✓.
    //   ShortCap: 1e9 < 3e10 ✓.
    //   UserCap: max_lendable * 2M * 23 / 10.8e9 = 3e10 * 4.6e7 / 1.08e10 ≈ 1.28e8 raw.
    //   1e9 > 1.28e8 → fails ✓.
    let (mut env, t, _) = migrated();
    let mut tr = env.get_treasury(&t);
    tr.lending_utilization_cap_bps = 1;
    env.poke_anchor(t.treasury, tr);

    let shorter = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.open_short(&shorter, &t, 2_000_000, MIN_SHORT_TOKENS),
        TorchMarketError::UserShortCapExceeded
    );
}

#[test]
fn open_short_via_vault_happy() {
    let (mut env, t, _) = migrated();
    let vault_owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&vault_owner);
    env.deposit_vault(&vault_owner, &vault, 2 * LAMPORTS_PER_SOL)
        .expect("fund");
    env.open_short_via_vault(&vault_owner, &vault, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open_short_via_vault");

    let pos = env.get_short(&t, &vault_owner.pubkey()).expect("short pos");
    assert_eq!(pos.tokens_borrowed, MIN_SHORT_TOKENS);
    let v = env.get_torch_vault(&vault.vault);
    assert_eq!(v.sol_balance, LAMPORTS_PER_SOL); // 2 - 1 collateral
    assert_eq!(v.total_spent, LAMPORTS_PER_SOL);
}

// ============================================================================
// close_short (6)
// ============================================================================

#[test]
fn close_short_partial() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open");

    // Close half of MIN_SHORT_TOKENS — sender has enough even after the
    // open-side transfer fee (got 1e9 - 700 raw ≈ 999.3M; sending 500M).
    env.close_short(&shorter, &t, MIN_SHORT_TOKENS / 2)
        .expect("partial close");
    let pos = env.get_short(&t, &shorter.pubkey()).expect("pos still");
    assert_eq!(pos.tokens_borrowed, MIN_SHORT_TOKENS / 2);
    assert_eq!(pos.sol_collateral, LAMPORTS_PER_SOL); // unchanged on partial
}

#[test]
fn close_short_full() {
    // Main's close_short zeroes tokens_borrowed on full close but does NOT
    // close the position PDA (dpi did — possible future port). Test the
    // observable invariant instead: full payoff drains debt and returns all
    // collateral.
    let (mut env, t, shorter) = migrated();
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open");

    env.close_short(&shorter, &t, MIN_SHORT_TOKENS * 2)
        .expect("full close");
    let pos = env.get_short(&t, &shorter.pubkey()).expect("pos still exists on main");
    assert_eq!(pos.tokens_borrowed, 0);
}

#[test]
fn close_short_interest_first() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open");

    // Warp to accrue interest in token terms.
    // calc_short_interest(1e9 raw, 200 bps, 1e6 slots) over
    //   EPOCH_DURATION_SLOTS (~1.512M slots) yields ~13M raw tokens accrued.
    env.warp_to_slot(env.current_slot() + 1_000_000);

    // Pay 1000 raw tokens — far below accrued interest, so entirely interest, no principal.
    env.close_short(&shorter, &t, 1000)
        .expect("interest-only close");
    let pos = env.get_short(&t, &shorter.pubkey()).expect("pos");
    assert_eq!(pos.tokens_borrowed, MIN_SHORT_TOKENS, "principal untouched");
    assert!(
        pos.accrued_interest > 0,
        "interest remains after small partial pay"
    );
}

#[test]
fn close_short_no_active_short() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    // Open a collateral-only short — context allows tokens_to_borrow=0
    // (EmptyBorrowRequest gate uses ||, not &&).
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, 0)
        .expect("open coll-only");

    expect_err!(
        env.close_short(&shorter, &t, 100),
        TorchMarketError::NoActiveShort
    );
}

#[test]
fn close_short_zero_amount() {
    let (mut env, t, _) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open");
    expect_err!(
        env.close_short(&shorter, &t, 0),
        TorchMarketError::ZeroAmount
    );
}

#[test]
fn close_short_via_vault_happy() {
    // Vault owner uses one of the bonding buyers (first_buyer) as the linked
    // signer so vault has access to extra tokens for the close-fee top-up.
    let (mut env, t, first_buyer) = migrated();
    let vault = env.create_vault(&first_buyer);
    env.deposit_vault(&first_buyer, &vault, 2 * LAMPORTS_PER_SOL)
        .expect("fund");

    // Move first_buyer's tokens into the vault ATA so close-side transfer has
    // a balance to draw from. Use a tiny transfer via withdraw_tokens... actually
    // simpler: just have the vault open a short and partially close.
    env.open_short_via_vault(&first_buyer, &vault, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open_short_via_vault");

    env.close_short_via_vault(&first_buyer, &vault, &t, MIN_SHORT_TOKENS / 2)
        .expect("close_short_via_vault");

    let pos = env.get_short(&t, &first_buyer.pubkey()).expect("pos");
    assert_eq!(pos.tokens_borrowed, MIN_SHORT_TOKENS / 2);
}

// ============================================================================
// liquidate_short (6)
// ============================================================================

#[test]
fn liquidate_short_happy() {
    // Open a near-max-LTV short. Pump pool_sol to push debt_value above
    // liquidation_threshold (65%). Liquidator covers half the debt.
    let (mut env, t, liquidator) = migrated(); // first_buyer has tokens
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);

    // 1 SOL collateral, 500k display tokens (5e11 raw) → LTV ≈ 33.4%
    let borrow_amount = 500_000_000_000;
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, borrow_amount)
        .expect("open");

    // Double pool_sol → debt_value doubles to ~0.668 SOL → LTV ≈ 66.8% > 65%.
    env.poke_pool_sol(&t, 200 * LAMPORTS_PER_SOL);

    env.liquidate_short(&liquidator, shorter.pubkey(), &t)
        .expect("liquidate_short");

    let pos = env.get_short(&t, &shorter.pubkey()).expect("pos");
    assert!(pos.tokens_borrowed < borrow_amount, "debt reduced");
    assert!(pos.sol_collateral < LAMPORTS_PER_SOL, "collateral seized");
}

#[test]
fn liquidate_short_not_liquidatable() {
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open"); // LTV 0.0668% — trivially healthy

    expect_err!(
        env.liquidate_short(&liquidator, shorter.pubkey(), &t),
        TorchMarketError::ShortNotLiquidatable
    );
}

#[test]
fn liquidate_short_partial_close_bps() {
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let borrow_amount = 500_000_000_000;
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, borrow_amount)
        .expect("open");
    let before = env.get_short(&t, &shorter.pubkey()).unwrap();

    env.poke_pool_sol(&t, 200 * LAMPORTS_PER_SOL);
    env.liquidate_short(&liquidator, shorter.pubkey(), &t)
        .expect("liquidate");

    let after = env.get_short(&t, &shorter.pubkey()).unwrap();
    let tokens_covered = before.tokens_borrowed - after.tokens_borrowed;
    // close_bps = 50%: at most half the debt covered per call.
    assert!(tokens_covered <= before.tokens_borrowed / 2 + 1);
    assert!(after.tokens_borrowed > 0, "still has debt after partial");
}

#[test]
fn liquidate_short_bad_debt() {
    // Push pool_sol so high that the SOL value of the FULL debt-to-cover
    // exceeds the entire collateral. Seizure caps at collateral_amount,
    // bad_debt > 0 written off.
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let borrow_amount = 500_000_000_000;
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, borrow_amount)
        .expect("open");

    // Pump pool_sol 10x → debt_value ~6.68 SOL, collateral 1 SOL.
    // Half of 6.68 = 3.34 SOL with 10% bonus = 3.67 SOL seizure target. > 1 SOL collateral → bad debt.
    env.poke_pool_sol(&t, 1000 * LAMPORTS_PER_SOL);
    env.liquidate_short(&liquidator, shorter.pubkey(), &t)
        .expect("liquidate");

    let pos = env.get_short(&t, &shorter.pubkey()).unwrap();
    assert_eq!(pos.sol_collateral, 0, "all collateral seized");
    assert_eq!(pos.accrued_interest, 0, "interest written off");
}

#[test]
fn liquidate_short_proceeds_when_pool_thin() {
    // dpi removed the pool-depth gate from liquidate_short; main still has it
    // (rejects with PoolTooThin before reaching the LTV check). When dpi's
    // depth-gate-removal is ported back, this test should assert
    // ShortNotLiquidatable like the original.
    let (mut env, t, liquidator) = migrated();
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, MIN_SHORT_TOKENS)
        .expect("open");
    env.poke_pool_sol(&t, 4 * LAMPORTS_PER_SOL);
    expect_err!(
        env.liquidate_short(&liquidator, shorter.pubkey(), &t),
        TorchMarketError::PoolTooThin
    );
}

#[test]
fn liquidate_short_via_vault_happy() {
    let (mut env, t, first_buyer) = migrated(); // first_buyer is the liquidator (has tokens)
    let shorter = env.new_funded(3 * LAMPORTS_PER_SOL);
    let borrow_amount = 500_000_000_000;
    env.open_short(&shorter, &t, LAMPORTS_PER_SOL, borrow_amount)
        .expect("open");

    let vault = env.create_vault(&first_buyer);
    env.deposit_vault(&first_buyer, &vault, LAMPORTS_PER_SOL)
        .expect("fund");
    // Vault needs tokens to cover the short debt. Move some from first_buyer to vault.
    // The simplest path: first_buyer sends tokens to vault ATA directly via the
    // Token-2022 program. We piggy-back on the auto-ATA-creation in the vault helpers
    // and just use a small short to keep numbers tractable.
    env.poke_pool_sol(&t, 200 * LAMPORTS_PER_SOL);

    // first_buyer (vault authority) signs the liquidation. The vault's ATA gets
    // auto-created. The vault's token balance is 0, so the transfer of tokens
    // from vault to treasury_lock will fail with insufficient funds — UNLESS
    // we first stage tokens in the vault.
    //
    // To stage: first_buyer transfers some of their 16M tokens to vault ATA.
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use torch_market::token_2022_utils::get_associated_token_address_2022;
    env.ensure_token2022_ata(&first_buyer, &vault.vault, &t.mint);
    let vault_ata = get_associated_token_address_2022(&vault.vault, &t.mint);
    let fb_ata = get_associated_token_address_2022(&first_buyer.pubkey(), &t.mint);
    let mut data = vec![12u8]; // TransferChecked discriminator
    data.extend_from_slice(&500_000_000_000u64.to_le_bytes());
    data.push(TOKEN_DECIMALS);
    let transfer_ix = Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(fb_ata, false),
            AccountMeta::new_readonly(t.mint, false),
            AccountMeta::new(vault_ata, false),
            AccountMeta::new_readonly(first_buyer.pubkey(), true),
        ],
        data,
    };
    env.send(&[transfer_ix], &[&first_buyer])
        .expect("stage tokens in vault");

    env.liquidate_short_via_vault(&first_buyer, &vault, shorter.pubkey(), &t)
        .expect("liquidate_short_via_vault");

    let v = env.get_torch_vault(&vault.vault);
    assert!(v.total_received > 0, "vault received seized SOL");
}
