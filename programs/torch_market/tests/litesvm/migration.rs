// migrate_to_dex tests — 8 cases covering happy path post-state, every reachable
// error variant on the migration path, and the LP burn / authority revoke
// invariants that production depends on.

use solana_sdk::{account::ReadableAccount, native_token::LAMPORTS_PER_SOL, signer::Signer};

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

fn setup_bonded() -> (Env, TokenCtx) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    env.bond_to_completion(&t);
    (env, t)
}

// ---------------------------------------------------------------------------

#[test]
fn happy_path_sets_migrated_flag() {
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);

    env.migrate(&t, &payer).expect("migrate");

    let bc = env.get_bonding_curve(&t);
    assert!(bc.migrated);
    assert_eq!(bc.real_sol_reserves, 0);
    assert_eq!(bc.real_token_reserves, 0);
}

#[test]
fn bonding_not_complete() {
    // Token created but not bonded.
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("buy");

    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    expect_err!(
        env.migrate(&t, &payer),
        TorchMarketError::BondingNotComplete
    );
}

#[test]
fn already_migrated() {
    let (mut env, t) = setup_bonded();
    let payer1 = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer1).expect("first migrate");

    let payer2 = env.new_funded(2 * LAMPORTS_PER_SOL);
    expect_err!(env.migrate(&t, &payer2), TorchMarketError::AlreadyMigrated);
}

#[test]
fn insufficient_migration_fee() {
    let (mut env, t) = setup_bonded();

    // Force treasury.sol_balance below MIN_MIGRATION_SOL via poke. Bonding
    // naturally accrues ~10 SOL to treasury; we starve it to test the
    // safety-net constraint.
    let mut tr = env.get_treasury(&t);
    tr.sol_balance = MIN_MIGRATION_SOL - 1;
    env.poke_anchor(t.treasury, tr);

    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    expect_err!(
        env.migrate(&t, &payer),
        TorchMarketError::InsufficientMigrationFee
    );
}

#[test]
fn baseline_recorded_post_migrate() {
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");

    let tr = env.get_treasury(&t);
    assert!(tr.baseline_initialized);
    assert!(tr.baseline_sol_reserves > 0);
    assert!(tr.baseline_token_reserves > 0);
    assert_eq!(
        tr.min_buyback_interval_slots,
        DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS
    );
}

#[test]
fn lp_fully_burned_for_payer_locked_for_pool() {
    use torch_market::token_2022_utils::get_associated_token_address_2022;

    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");

    let payer_lp = get_associated_token_address_2022(&payer.pubkey(), &t.deep_pool_lp_mint);
    let pool_lp = get_associated_token_address_2022(&t.deep_pool, &t.deep_pool_lp_mint);

    let payer_amt = read_token_amount(&env, &payer_lp);
    let pool_amt = read_token_amount(&env, &pool_lp);

    assert_eq!(payer_amt, 0, "payer's LP must be fully burned");
    assert!(pool_amt > 0, "pool's locked LP (20%) must be present");
}

#[test]
fn mint_authority_revoked() {
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");

    // Token-2022 mint layout: bytes 0..4 = mint_authority COption discriminator.
    // [0,0,0,0] = None.
    let mint_acct = env.svm.get_account(&t.mint).expect("mint");
    let data = mint_acct.data();
    assert_eq!(
        &data[0..4],
        &[0u8, 0, 0, 0],
        "mint_authority must be None after migration"
    );
    // freeze_authority at bytes 46..50 — was None at creation, still None.
    assert_eq!(&data[46..50], &[0u8, 0, 0, 0]);
}

#[test]
fn payer_net_loss_only_ata_rent_and_tx_fee() {
    // Sanity check on the SOL flow:
    //   - fund_migration_sol credits payer with `real_sol_reserves`
    //   - create_pool inside migrate_to_dex moves that SOL into the deep_pool PDA
    //   - migrate_to_dex reimburses payer for rent of pool-side accounts
    // Net: payer only pays for the payer_token ATA (~0.002 SOL) + tx fee (~5k lamports).
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    let payer_before = env.svm.get_account(&payer.pubkey()).unwrap().lamports;
    let curve_sol = env.get_bonding_curve(&t).real_sol_reserves;

    env.migrate(&t, &payer).expect("migrate");

    let payer_after = env.svm.get_account(&payer.pubkey()).unwrap().lamports;
    let loss = payer_before - payer_after;
    // Should be at most a few cents — pure overhead, nothing fundamental.
    assert!(
        loss < 10_000_000, // 0.01 SOL ceiling
        "payer lost {} lamports (~{:.4} SOL); expected near-zero overhead",
        loss,
        loss as f64 / LAMPORTS_PER_SOL as f64
    );

    // The bonding curve's SOL must have flowed into the deep_pool PDA.
    let deep_pool_lamports = env.svm.get_account(&t.deep_pool).unwrap().lamports;
    assert!(
        deep_pool_lamports >= curve_sol,
        "deep_pool {} < curve_sol {}",
        deep_pool_lamports,
        curve_sol
    );
}

// ---------------------------------------------------------------------------

fn read_token_amount(env: &Env, ata: &solana_sdk::pubkey::Pubkey) -> u64 {
    let acct = env
        .svm
        .get_account(ata)
        .unwrap_or_else(|| panic!("ATA {} missing", ata));
    let data = acct.data();
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}
