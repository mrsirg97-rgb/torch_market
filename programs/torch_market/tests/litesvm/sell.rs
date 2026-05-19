// Sell handler tests — 6 cases covering happy paths and reachable errors.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signer::Signer};

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

fn token_with_buyer(env: &mut Env) -> (TokenCtx, solana_sdk::signature::Keypair) {
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("seed buy");
    (t, buyer)
}

fn read_token_balance(
    env: &Env,
    owner: &solana_sdk::pubkey::Pubkey,
    mint: &solana_sdk::pubkey::Pubkey,
) -> u64 {
    use solana_sdk::account::ReadableAccount;
    use torch_market::token_2022_utils::get_associated_token_address_2022;
    let ata = get_associated_token_address_2022(owner, mint);
    let acct = env.svm.get_account(&ata).expect("ATA missing");
    let data = acct.data();
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

// ---------------------------------------------------------------------------

#[test]
fn happy_path() {
    let mut env = Env::new();
    let (t, seller) = token_with_buyer(&mut env);

    let bal = read_token_balance(&env, &seller.pubkey(), &t.mint);
    let bc_sol_before = env.get_bonding_curve(&t).real_sol_reserves;

    env.sell(&seller, &t, bal / 2, 0).expect("sell");

    let bc_sol_after = env.get_bonding_curve(&t).real_sol_reserves;
    assert!(
        bc_sol_after < bc_sol_before,
        "curve SOL should decrease on sell"
    );
    let new_bal = read_token_balance(&env, &seller.pubkey(), &t.mint);
    assert!(new_bal < bal);
}

#[test]
fn zero_amount() {
    let mut env = Env::new();
    let (t, seller) = token_with_buyer(&mut env);
    expect_err!(env.sell(&seller, &t, 0, 0), TorchMarketError::ZeroAmount);
}

#[test]
fn bonding_complete_blocks_sell() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let seller = env.bond_to_completion(&t);

    // Token-2022 transfer-fee leaves seller with slightly less than face value
    // anyway; we just need >0. Try to sell 1 token unit.
    expect_err!(
        env.sell(&seller, &t, 1_000_000, 0),
        TorchMarketError::BondingComplete
    );
}

#[test]
fn insufficient_tokens() {
    let mut env = Env::new();
    let (t, seller) = token_with_buyer(&mut env);
    let bal = read_token_balance(&env, &seller.pubkey(), &t.mint);
    // Try to sell more than owned. Token-2022 transfer fails with
    // InstructionError::Custom(1) (SPL InsufficientFunds), NOT an Anchor variant.
    // The Anchor check `seller_token_account.amount >= args.token_amount` is in
    // the handler — try selling 2x balance.
    expect_err!(
        env.sell(&seller, &t, bal * 2, 0),
        TorchMarketError::InsufficientTokens
    );
}

#[test]
fn sell_via_vault_happy() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);

    // Vault buys, then vault sells. Vault owner is the linked signer.
    let vault_owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&vault_owner);
    env.deposit_vault(&vault_owner, &vault, 2 * LAMPORTS_PER_SOL)
        .expect("deposit");
    env.buy_via_vault(&vault_owner, &vault, &t, 100_000_000, 0)
        .expect("buy_via_vault");

    let vault_balance_pre = env.get_torch_vault(&vault.vault).sol_balance;
    let vault_tokens = read_token_balance(&env, &vault.vault, &t.mint);
    assert!(vault_tokens > 0);

    env.sell_via_vault(&vault_owner, &vault, &t, vault_tokens / 2, 0)
        .expect("sell_via_vault");

    let v = env.get_torch_vault(&vault.vault);
    assert!(
        v.sol_balance > vault_balance_pre,
        "vault should gain SOL from sell"
    );
    assert!(v.total_received > 0);
}

#[test]
fn sell_after_reclaim_rejected() {
    // Sell context now has explicit `!reclaimed` constraint (defense in depth
    // over relying on `InsufficientSol` from compute_sell). Sells on reclaimed
    // tokens fail at the constraint layer.
    let mut env = Env::new();
    let (t, seller) = token_with_buyer(&mut env);

    env.warp_to_slot(env.current_slot() + INACTIVITY_PERIOD_SLOTS + 1);
    let reclaimer = env.new_funded(LAMPORTS_PER_SOL);
    env.reclaim_failed_token(&reclaimer, &t).expect("reclaim");
    assert!(env.get_bonding_curve(&t).reclaimed);

    let bal = read_token_balance(&env, &seller.pubkey(), &t.mint);
    expect_err!(
        env.sell(&seller, &t, bal / 2, 0),
        TorchMarketError::AlreadyReclaimed
    );
}
