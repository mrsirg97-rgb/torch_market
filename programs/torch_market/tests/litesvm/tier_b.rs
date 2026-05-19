// Tier B regression tests. These were authored to PIN pre-fix behavior, then
// flipped as each Tier B fix landed. Now they assert the FIXED behavior:
//   1. liquidation_proceeds_when_pool_thin  — depth gate removed from liquidate.
//   2. loan_position_closes_on_full_repay   — PDA closed, rent refunded.
//   3. short_position_closes_on_full_close  — PDA closed, rent refunded.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signer::Signer};

use crate::harness::{Env, TokenCtx};
use torch_market::constants::*;

// Shared setup: token + bond + migrate. Returns env, token ctx, and the
// "last bonding buyer" — who holds tokens we can use as borrower collateral.
fn migrated_token() -> (Env, TokenCtx, solana_sdk::signature::Keypair) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let last_buyer = env.bond_to_completion(&t);
    let migrator = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &migrator).expect("migrate");
    assert!(env.get_bonding_curve(&t).migrated);
    (env, t, last_buyer)
}

// ---------------------------------------------------------------------------
// 1. liquidation_proceeds_when_pool_thin
//
// Tier B fix: the depth gate (`require_min_pool_liquidity`) was removed from the
// liquidate paths. Existing positions must be liquidatable even when pool depth
// has collapsed, otherwise bad debt sits stranded.
// ---------------------------------------------------------------------------

#[test]
fn liquidation_proceeds_when_pool_thin() {
    let (mut env, t, borrower) = migrated_token();

    let bal = read_borrower_token_balance(&env, &t, &borrower);
    assert!(bal > 1_000_000, "borrower has no tokens after bonding");
    let collateral = bal / 2;
    let borrow_amount = 200_000_000;
    env.borrow(&borrower, &t, collateral, borrow_amount)
        .expect("borrow");

    // Drain pool below MIN_POOL_SOL_LENDING.
    env.poke_pool_sol(&t, MIN_POOL_SOL_LENDING - 1);

    let liquidator = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.liquidate(&liquidator, borrower.pubkey(), &t)
        .expect("liquidation must succeed even with thin pool");
}

// ---------------------------------------------------------------------------
// 2. loan_position_closes_on_full_repay
//
// Tier B fix: handlers now call `close_account_to` after full repay, refunding
// the loan_position PDA's rent to the borrower and freeing the slot for re-init.
// ---------------------------------------------------------------------------

#[test]
fn loan_position_closes_on_full_repay() {
    let (mut env, t, borrower) = migrated_token();

    let bal = read_borrower_token_balance(&env, &t, &borrower);
    let borrow_amount = 200_000_000;
    env.borrow(&borrower, &t, bal / 2, borrow_amount)
        .expect("borrow");

    let (loan_addr, _) = solana_sdk::pubkey::Pubkey::find_program_address(
        &[LOAN_SEED, t.mint.as_ref(), borrower.pubkey().as_ref()],
        &torch_market::ID,
    );
    assert!(
        env.account_exists(&loan_addr),
        "loan PDA exists during borrow"
    );
    let borrower_before = env.svm.get_account(&borrower.pubkey()).unwrap().lamports;

    env.repay(&borrower, &t, borrow_amount * 10).expect("repay");

    assert!(
        !env.account_exists(&loan_addr),
        "loan PDA should be closed after full repay"
    );
    let borrower_after = env.svm.get_account(&borrower.pubkey()).unwrap().lamports;
    // After full repay: borrower paid `borrow_amount` of debt back, but also
    // received the loan_position rent refund AND the collateral. Net should
    // exceed the borrow_amount they sent out by ~rent.
    let net = borrower_after as i128 - borrower_before as i128;
    assert!(
        net > -(borrow_amount as i128),
        "rent refund offset some of the repay (net change: {})",
        net
    );
}

// ---------------------------------------------------------------------------
// 3. short_position_leaks_on_full_close
//
// Same shape as #2 but for shorts. Open short, close fully, assert position
// PDA persists with zeroed fields.
// ---------------------------------------------------------------------------

#[test]
fn short_position_closes_on_full_close() {
    // Tier B fix: short_position PDA is closed on full close, rent returned
    // to shorter. Use a bonding-buyer (first_buyer) so they have spare tokens
    // to cover the Token-2022 fee on the close-side transfer.
    let (mut env, t, shorter) = migrated_token();
    env.airdrop(&shorter.pubkey(), 2 * LAMPORTS_PER_SOL);

    let sol_collateral = LAMPORTS_PER_SOL;
    let tokens_to_borrow = MIN_SHORT_TOKENS;
    env.open_short(&shorter, &t, sol_collateral, tokens_to_borrow)
        .expect("open_short");

    let (short_addr, _) = solana_sdk::pubkey::Pubkey::find_program_address(
        &[SHORT_SEED, t.mint.as_ref(), shorter.pubkey().as_ref()],
        &torch_market::ID,
    );
    assert!(
        env.account_exists(&short_addr),
        "short PDA exists during open"
    );

    env.close_short(&shorter, &t, tokens_to_borrow * 2)
        .expect("close_short");

    assert!(
        !env.account_exists(&short_addr),
        "short PDA should be closed after full close"
    );
    assert!(
        env.get_short(&t, &shorter.pubkey()).is_none(),
        "get_short returns None after close"
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn read_borrower_token_balance(
    env: &Env,
    t: &TokenCtx,
    borrower: &solana_sdk::signature::Keypair,
) -> u64 {
    use solana_sdk::account::ReadableAccount;
    use torch_market::token_2022_utils::get_associated_token_address_2022;
    let ata = get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
    let acct = env.svm.get_account(&ata).expect("borrower ATA missing");
    // SPL token account layout: mint(32) + owner(32) + amount(8) = amount at offset 64.
    let data = acct.data();
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}
