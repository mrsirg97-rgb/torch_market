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
fn lp_fully_burned_for_payer() {
    // Main burns 100% of the payer's LP at migration (see migration.rs Burn CPI).
    // dpi splits LP 80/20 (burn payer, lock pool) via DeepPool; main is Raydium,
    // which has no equivalent locked-pool concept. Verify the payer-burn half only.
    use solana_sdk::pubkey::Pubkey;
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &payer).expect("migrate");

    // Derive Raydium LP mint, then the payer's SPL-Token ATA for it.
    let raydium: Pubkey = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C".parse().unwrap();
    let amm_config: Pubkey = "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2".parse().unwrap();
    let wsol: Pubkey = "So11111111111111111111111111111111111111112".parse().unwrap();
    let (token_0, token_1) = if wsol < t.mint { (wsol, t.mint) } else { (t.mint, wsol) };
    let (pool_state, _) = Pubkey::find_program_address(
        &[b"pool", amm_config.as_ref(), token_0.as_ref(), token_1.as_ref()],
        &raydium,
    );
    let (lp_mint, _) =
        Pubkey::find_program_address(&[b"pool_lp_mint", pool_state.as_ref()], &raydium);
    let spl_token: Pubkey = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap();
    let ata_program = torch_market::token_2022_utils::ASSOCIATED_TOKEN_PROGRAM_ID;
    let (payer_lp, _) = Pubkey::find_program_address(
        &[payer.pubkey().as_ref(), spl_token.as_ref(), lp_mint.as_ref()],
        &ata_program,
    );

    assert_eq!(read_token_amount(&env, &payer_lp), 0, "payer LP fully burned");
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
fn payer_net_loss_capped_by_raydium_pool_creation() {
    // SOL flow under Raydium migration:
    //   1. fund_migration_wsol moves real_sol_reserves into the bonding_curve's
    //      WSOL ATA (still owned by bonding_curve).
    //   2. migrate_to_dex closes bc_wsol → payer_wsol (rent + wrapped SOL).
    //   3. Raydium initialize CPI pulls payer_wsol into the pool's WSOL vault,
    //      and charges create_pool_fee (~0.15 SOL on mainnet; less on litesvm
    //      since the fee receiver is just an SOL sink).
    //   4. migrate_to_dex reimburses the payer for `migration_cost` from
    //      treasury, so net cost ≈ payer_token ATA rent + tx fee.
    // Verifying the cap, not the exact value (varies with Raydium fee config).
    let (mut env, t) = setup_bonded();
    let payer = env.new_funded(5 * LAMPORTS_PER_SOL);
    let payer_before = env.svm.get_account(&payer.pubkey()).unwrap().lamports;

    env.migrate(&t, &payer).expect("migrate");

    let payer_after = env.svm.get_account(&payer.pubkey()).unwrap().lamports;
    // Treasury reimburses the migration cost. Payer should net very little loss.
    // Allowing 0.2 SOL of headroom — significant overhead would indicate a
    // reimbursement regression.
    let loss = payer_before.saturating_sub(payer_after);
    assert!(
        loss < 200_000_000,
        "payer lost {} lamports (~{:.4} SOL); expected < 0.2 SOL after treasury reimbursement",
        loss,
        loss as f64 / LAMPORTS_PER_SOL as f64
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
