// contribute_revival tests — 4 cases. After a token is reclaimed, anyone can
// contribute SOL. Cumulative contributions >= IVS for the bonding tier flip
// `reclaimed=false`, returning the token to active trading.

use solana_sdk::native_token::LAMPORTS_PER_SOL;

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

fn reclaimed_token(env: &mut Env) -> TokenCtx {
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed buy");
    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.reclaim_failed_token(&payer, &t).expect("reclaim");
    t
}

// ---------------------------------------------------------------------------

#[test]
fn happy_path_below_threshold() {
    let mut env = Env::new();
    let t = reclaimed_token(&mut env);

    let contributor = env.new_funded(LAMPORTS_PER_SOL);
    let amount = 500_000_000; // 0.5 SOL — above MIN_SOL_AMOUNT, below FLAME IVS (37.5 SOL)
    env.contribute_revival(&contributor, &t, amount)
        .expect("contribute");

    let bc = env.get_bonding_curve(&t);
    assert!(bc.reclaimed, "should still be reclaimed (below threshold)");
    assert_eq!(bc.real_sol_reserves, amount);
}

#[test]
fn token_not_reclaimed() {
    // Try to contribute to a token that hasn't been reclaimed.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);

    let contributor = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.contribute_revival(&contributor, &t, 500_000_000),
        TorchMarketError::TokenNotReclaimed
    );
}

#[test]
fn amount_too_small() {
    let mut env = Env::new();
    let t = reclaimed_token(&mut env);

    let contributor = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.contribute_revival(&contributor, &t, MIN_SOL_AMOUNT - 1),
        TorchMarketError::AmountTooSmall
    );
}

#[test]
fn threshold_triggers_revival() {
    let mut env = Env::new();
    let t = reclaimed_token(&mut env);

    // For FLAME (target 100 SOL), initial_virtual_reserves returns 37.5 SOL.
    // The threshold is that IVS. Contribute slightly above.
    //
    // Cumulative `real_sol_reserves` after reclaim starts at 0 (reset by reclaim),
    // so we need the single contribution to clear the threshold.
    let threshold = 37_500_000_000_u64; // FLAME IVS
    let contributor = env.new_funded(40 * LAMPORTS_PER_SOL);
    env.contribute_revival(&contributor, &t, threshold + LAMPORTS_PER_SOL)
        .expect("contribute over threshold");

    let bc = env.get_bonding_curve(&t);
    assert!(!bc.reclaimed, "token should be revived (reclaimed = false)");
    assert!(bc.real_sol_reserves >= threshold);
}
