// Token-treasury tests (harvest_fees + swap_fees_to_sol).
//
// swap_fees_to_sol semantics (verified against handlers/treasury.rs):
//   - Returns Ok(()) WITHOUT swapping if cooldown not elapsed OR price below
//     1.2× baseline. Not an error — the gate is silent.
//   - On happy path: sells DEFAULT_SELL_PERCENT_BPS (15%) of treasury tokens,
//     splits 15% creator / 85% treasury (or 0/100 for community tokens),
//     updates last_buyback_slot.

use solana_sdk::{account::ReadableAccount, native_token::LAMPORTS_PER_SOL, signature::Keypair};

use crate::harness::{Env, TokenCtx};
use torch_market::constants::*;

fn migrated() -> (Env, TokenCtx, Keypair) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let first_buyer = env.bond_to_completion(&t);
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");
    (env, t, first_buyer)
}

#[test]
fn harvest_fees_empty_sources_is_noop() {
    // harvest_fees with no remaining_accounts skips the harvest step and only
    // runs withdraw_withheld_tokens_from_mint. With no withheld balance the
    // withdraw is also a no-op but should succeed.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.harvest_fees(&payer, &t, &[]).expect("harvest empty");
}

#[test]
fn swap_fees_to_sol_below_threshold_is_silent_noop() {
    // Pool exactly at baseline (ratio = 1.0x) — below the 1.2x threshold.
    // Handler should return Ok without actually swapping; treasury.sol_balance
    // unchanged.
    let (mut env, t, _) = migrated();
    env.poke_token_amount(t.treasury_token_account, 10_000_000_000); // 10M raw

    let tr_before = env.get_treasury(&t);
    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.swap_fees_to_sol(&payer, &t, 1)
        .expect("returns ok, no swap");
    let tr_after = env.get_treasury(&t);
    assert_eq!(
        tr_before.sol_balance, tr_after.sol_balance,
        "no swap should occur"
    );
    assert_eq!(tr_before.last_buyback_slot, tr_after.last_buyback_slot);
}

#[test]
fn swap_fees_to_sol_cooldown_is_silent_noop() {
    // Set last_buyback_slot to current; min_buyback_interval_slots already 2700.
    // current_slot < next_slot → handler returns Ok early without checking ratio
    // or doing any work.
    let (mut env, t, _) = migrated();
    let current = env.current_slot();
    let mut tr = env.get_treasury(&t);
    tr.last_buyback_slot = current;
    env.poke_anchor(t.treasury, tr);
    env.poke_token_amount(t.treasury_token_account, 10_000_000_000);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.swap_fees_to_sol(&payer, &t, 1).expect("cooldown no-op");

    let tr_after = env.get_treasury(&t);
    assert_eq!(tr_after.sol_balance, env.get_treasury(&t).sol_balance);
    assert_eq!(tr_after.last_buyback_slot, current); // unchanged
}

#[test]
fn swap_fees_to_sol_happy_splits_creator_85_15() {
    // Pump pool price 2× baseline, give treasury tokens, run the swap. Verify
    // creator received 15% of SOL proceeds and treasury.sol_balance got 85%.
    let (mut env, t, _) = migrated();

    // baseline_sol_reserves was set to the curve_sol at migration; pump pool 2x.
    let baseline_sol = env.get_treasury(&t).baseline_sol_reserves;
    env.poke_pool_sol(&t, baseline_sol * 2);

    // Stage 100M raw tokens in treasury_token_account. (Production accrual
    // path is harvest_fees, but for unit purposes we set it directly.)
    let staged: u64 = 100_000_000_000_000; // 100M display tokens raw
    env.poke_token_amount(t.treasury_token_account, staged);

    let creator_before = env
        .svm
        .get_account(&t.creator)
        .map(|a| a.lamports())
        .unwrap_or(0);
    let tr_before = env.get_treasury(&t);

    let payer = env.new_funded(LAMPORTS_PER_SOL);
    env.swap_fees_to_sol(&payer, &t, 1).expect("happy swap");

    let creator_after = env
        .svm
        .get_account(&t.creator)
        .map(|a| a.lamports())
        .unwrap_or(0);
    let tr_after = env.get_treasury(&t);
    let creator_gained = creator_after - creator_before;
    let treasury_gained = tr_after.sol_balance - tr_before.sol_balance;

    assert!(creator_gained > 0, "creator gained SOL");
    assert!(treasury_gained > 0, "treasury gained SOL");
    // Split is 15/85; allow ±1 lamport for rounding.
    let total = creator_gained + treasury_gained;
    let expected_creator = total * CREATOR_FEE_SHARE_BPS as u64 / 10000;
    assert!(
        (creator_gained as i128 - expected_creator as i128).abs() <= 1,
        "expected creator share ≈ {} got {}",
        expected_creator,
        creator_gained
    );
    assert!(tr_after.last_buyback_slot >= tr_before.last_buyback_slot);
}
