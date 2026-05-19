// star_token tests — 3 cases (happy / CannotStarSelf / creator payout at threshold).

use solana_sdk::native_token::LAMPORTS_PER_SOL;

use crate::{expect_err, harness::Env};
use torch_market::{constants::*, errors::TorchMarketError};

#[test]
fn star_token_happy() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);

    let user = env.new_funded(LAMPORTS_PER_SOL);
    env.star_token(&user, &t).expect("star");

    let tr = env.get_treasury(&t);
    assert_eq!(tr.total_stars, 1);
    assert_eq!(tr.star_sol_balance, STAR_COST_LAMPORTS);
    assert!(!tr.creator_paid_out);
}

#[test]
fn star_self_rejected() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    expect_err!(
        env.star_token(&creator, &t),
        TorchMarketError::CannotStarSelf
    );
}

#[test]
fn creator_payout_triggers_at_threshold() {
    // Avoid 2000 real ix calls: poke treasury.total_stars to threshold-1, then
    // do one real star → payout triggers.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);

    let mut tr = env.get_treasury(&t);
    tr.total_stars = CREATOR_REWARD_THRESHOLD - 1;
    // Simulate accumulated star payments (would have come from ~1999 real stars).
    tr.star_sol_balance = STAR_COST_LAMPORTS * (CREATOR_REWARD_THRESHOLD - 1);
    env.poke_anchor(t.treasury, tr);

    // Treasury PDA needs the matching lamports to back the payout.
    env.airdrop(
        &t.treasury,
        STAR_COST_LAMPORTS * (CREATOR_REWARD_THRESHOLD - 1),
    );

    let creator_before = env.svm.get_account(&t.creator).unwrap().lamports;
    let user = env.new_funded(LAMPORTS_PER_SOL);
    env.star_token(&user, &t).expect("final star");

    let creator_after = env.svm.get_account(&t.creator).unwrap().lamports;
    let tr = env.get_treasury(&t);
    assert!(tr.creator_paid_out, "creator should be paid out");
    assert_eq!(tr.star_sol_balance, 0, "balance drained on payout");
    assert!(
        creator_after > creator_before,
        "creator received SOL ({} → {})",
        creator_before,
        creator_after
    );
}
