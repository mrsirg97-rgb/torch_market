// reclaim_failed_token tests — 5 cases. Reclaim moves SOL from the bonding
// curve PDA and token treasury into the protocol_treasury after the token has
// been inactive >= 7 days and not completed bonding.

use solana_sdk::native_token::LAMPORTS_PER_SOL;

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

fn fresh_token(env: &mut Env) -> TokenCtx {
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.create_token(&creator, BONDING_TARGET_FLAME, false)
}

// ---------------------------------------------------------------------------

#[test]
fn happy_path() {
    let mut env = Env::new();
    let t = fresh_token(&mut env);
    // Seed the curve so total_sol clears MIN_RECLAIM_THRESHOLD.
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed buy");

    let curve_sol_before = env.get_bonding_curve(&t).real_sol_reserves;
    let treasury_sol_before = env.get_treasury(&t).sol_balance;
    let pt_before = env.get_protocol_treasury().total_fees_received;
    let expected = curve_sol_before + treasury_sol_before;

    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.reclaim_failed_token(&payer, &t).expect("reclaim");

    let bc = env.get_bonding_curve(&t);
    assert!(bc.reclaimed);
    assert_eq!(bc.real_sol_reserves, 0);
    assert_eq!(env.get_treasury(&t).sol_balance, 0);
    let pt_after = env.get_protocol_treasury().total_fees_received;
    assert_eq!(pt_after - pt_before, expected);
}

#[test]
fn token_still_active() {
    // Activity recent → reclaim refuses.
    let mut env = Env::new();
    let t = fresh_token(&mut env);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed buy");

    // No warp — still within activity window.
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.reclaim_failed_token(&payer, &t),
        TorchMarketError::TokenStillActive
    );
}

#[test]
fn already_reclaimed() {
    let mut env = Env::new();
    let t = fresh_token(&mut env);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed buy");
    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.reclaim_failed_token(&payer, &t).expect("first reclaim");

    // Second attempt — context constraint `!bonding_curve.reclaimed` fires.
    let payer2 = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.reclaim_failed_token(&payer2, &t),
        TorchMarketError::AlreadyReclaimed
    );
}

#[test]
fn below_reclaim_threshold() {
    // No buys → curve.real_sol_reserves = 0, treasury.sol_balance = 0,
    // total = 0 < MIN_RECLAIM_THRESHOLD (0.01 SOL).
    let mut env = Env::new();
    let t = fresh_token(&mut env);
    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.reclaim_failed_token(&payer, &t),
        TorchMarketError::BelowReclaimThreshold
    );
}

#[test]
fn bonded_token_blocks_reclaim() {
    let mut env = Env::new();
    let t = fresh_token(&mut env);
    env.bond_to_completion(&t);
    // Even if we warp 7 days — the context's `!bonding_complete` constraint
    // fires before the activity check.
    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.reclaim_failed_token(&payer, &t),
        TorchMarketError::BondingComplete
    );
}
