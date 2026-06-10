// protocol_treasury: epoch lifecycle + claim_protocol_rewards.
//
// Note on the claim gate: `compute_claim` requires `last_epoch_claimed < claimable_epoch`
// where `claimable_epoch = current_epoch - 1`. Default last_epoch_claimed = 0 and
// saturating_sub(1) make epoch=1 unreachable. Tests therefore set up state at
// epoch=2 (one advance for state machine, second to make claimable_epoch=1).

use solana_sdk::{
    account::ReadableAccount, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, signature::Keypair,
    signer::Signer,
};

use crate::{expect_err, harness::Env};
use torch_market::{constants::*, errors::TorchMarketError, state::UserStats};

#[test]
fn advance_epoch_too_early() {
    let mut env = Env::new();
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.advance_protocol_epoch(&payer),
        TorchMarketError::EpochNotComplete
    );
}

#[test]
fn advance_epoch_happy() {
    let mut env = Env::new();
    env.advance_time(EPOCH_DURATION_SECONDS + 1);
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.advance_protocol_epoch(&payer).expect("advance");
    assert_eq!(env.get_protocol_treasury().current_epoch, 1);
}

#[test]
fn claim_no_rewards_when_epoch_zero() {
    // current_epoch = 0 → compute_claim's `current_epoch > 0` gate fires.
    let mut env = Env::new();
    let user = init_user_stats(&mut env);
    expect_err!(
        env.claim_protocol_rewards(&user),
        TorchMarketError::NoRewardsAvailable
    );
}

#[test]
fn claim_happy_path() {
    let mut env = Env::new();
    let user = init_user_stats(&mut env);
    setup_claimable_state(&mut env, &user, 3 * LAMPORTS_PER_SOL);

    let user_before = env.svm.get_account(&user.pubkey()).unwrap().lamports;
    env.claim_protocol_rewards(&user).expect("claim");
    let user_after = env.svm.get_account(&user.pubkey()).unwrap().lamports;
    assert!(user_after > user_before, "user received SOL from claim");
}

#[test]
fn claim_insufficient_volume() {
    let mut env = Env::new();
    let user = init_user_stats(&mut env);
    // Volume just under the eligibility floor.
    setup_claimable_state(&mut env, &user, MIN_EPOCH_VOLUME_ELIGIBILITY - 1);
    expect_err!(
        env.claim_protocol_rewards(&user),
        TorchMarketError::InsufficientVolumeForRewards
    );
}

#[test]
fn claim_already_claimed() {
    let mut env = Env::new();
    let user = init_user_stats(&mut env);
    setup_claimable_state(&mut env, &user, 3 * LAMPORTS_PER_SOL);

    env.claim_protocol_rewards(&user).expect("first claim");
    expect_err!(
        env.claim_protocol_rewards(&user),
        TorchMarketError::AlreadyClaimed
    );
}

// [F-7] Claims are pro-rata against the EPOCH SNAPSHOT, not the live
// spend-down ledger — equal-volume claimers receive equal payouts regardless
// of claim order. Pre-fix, the share base shrank after every claim: the first
// equal claimer got cap×D, the second cap×(D − paid) — geometric decay. Both
// claims here also exercise the 10% anti-monopoly cap ON THE SNAPSHOT (each
// user holds 50% of volume, so the cap binds at 0.1×snapshot for both).
#[test]
fn claims_are_order_independent_across_equal_claimers() {
    let mut env = Env::new();
    let user_a = init_user_stats(&mut env);
    let user_b = init_user_stats(&mut env);
    let volume = 3 * LAMPORTS_PER_SOL;

    // Shared treasury state: epoch 2, total volume = both users combined.
    let pt_addr = env.protocol_treasury;
    let payout_pool: u64 = 2 * LAMPORTS_PER_SOL;
    env.airdrop(&pt_addr, payout_pool);
    let mut pt = env.get_protocol_treasury();
    pt.current_epoch = 2;
    pt.total_volume_previous_epoch = 2 * volume;
    pt.current_balance = payout_pool;
    pt.distributable_amount = payout_pool;
    pt.epoch_distributable_snapshot = payout_pool;
    env.poke_anchor(pt_addr, pt);

    for user in [&user_a, &user_b] {
        let (us_addr, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, user.pubkey().as_ref()],
            &torch_market::ID,
        );
        let acct = env.svm.get_account(&us_addr).expect("user_stats");
        use anchor_lang::AccountDeserialize;
        let mut data_view: &[u8] = acct.data();
        let mut stats = UserStats::try_deserialize(&mut data_view).expect("deser");
        stats.last_volume_epoch = 1;
        stats.volume_current_epoch = volume;
        stats.total_volume = stats.total_volume.max(volume);
        env.poke_anchor(us_addr, stats);
    }

    let a_before = env.svm.get_account(&user_a.pubkey()).unwrap().lamports;
    env.claim_protocol_rewards(&user_a).expect("claim a");
    let a_delta = env.svm.get_account(&user_a.pubkey()).unwrap().lamports - a_before;

    let b_before = env.svm.get_account(&user_b.pubkey()).unwrap().lamports;
    env.claim_protocol_rewards(&user_b).expect("claim b");
    let b_delta = env.svm.get_account(&user_b.pubkey()).unwrap().lamports - b_before;

    assert_eq!(a_delta, b_delta, "equal volume → equal payout, any order");
    // Cap applied to the SNAPSHOT: each 50%-volume user clamps at 10% of it.
    // delta = claim − 5000-lamport tx fee (the claimer signs their own claim).
    let expected_claim = payout_pool * MAX_CLAIM_SHARE_BPS / 10_000;
    assert_eq!(a_delta, expected_claim - 5_000, "cap binds on the snapshot");
}

// ---------------------------------------------------------------------------

/// Initialize a user with a small buy so their `user_stats` PDA exists.
fn init_user_stats(env: &mut Env) -> Keypair {
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let user = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.buy(&user, &t, 100_000_000, 0).expect("init user_stats");
    user
}

/// Poke env into a state where `user` can successfully claim:
///   protocol_treasury.current_epoch = 2 → claimable_epoch = 1
///   total_volume_previous_epoch = volume → denominator OK
///   distributable_amount = 1 SOL (and lamports backing it)
///   user_stats.last_volume_epoch = 1 → roll on next claim
///   user_stats.volume_current_epoch = volume → after roll, volume_previous_epoch = volume
fn setup_claimable_state(env: &mut Env, user: &Keypair, volume: u64) {
    // Stash protocol_treasury address before mutable borrows.
    let pt_addr = env.protocol_treasury;
    let payout_pool: u64 = LAMPORTS_PER_SOL;
    env.airdrop(&pt_addr, payout_pool);

    let mut pt = env.get_protocol_treasury();
    pt.current_epoch = 2;
    pt.total_volume_previous_epoch = volume;
    pt.current_balance = payout_pool;
    pt.distributable_amount = payout_pool;
    pt.epoch_distributable_snapshot = payout_pool; // [F-7] frozen share base
    env.poke_anchor(pt_addr, pt);

    let (us_addr, _) = Pubkey::find_program_address(
        &[USER_STATS_SEED, user.pubkey().as_ref()],
        &torch_market::ID,
    );
    let acct = env.svm.get_account(&us_addr).expect("user_stats");
    use anchor_lang::AccountDeserialize;
    let mut data_view: &[u8] = acct.data();
    let mut stats = UserStats::try_deserialize(&mut data_view).expect("deser");
    stats.last_volume_epoch = 1;
    stats.volume_current_epoch = volume;
    stats.total_volume = stats.total_volume.max(volume);
    env.poke_anchor(us_addr, stats);
}
