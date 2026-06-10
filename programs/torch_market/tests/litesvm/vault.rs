// Vault tests — 6 cases covering lifecycle (create, deposit, withdraw,
// link/unlink, authority transfer) and reachable errors (VaultUnauthorized,
// VaultWalletLinkMismatch, ZeroAmount on deposit, InsufficientVaultBalance).

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signature::Keypair, signer::Signer};

use crate::{expect_err, harness::Env};
use torch_market::errors::TorchMarketError;

#[test]
fn create_deposit_withdraw_lifecycle() {
    let mut env = Env::new();
    let creator = env.new_funded(3 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);

    let v = env.get_torch_vault(&vault.vault);
    assert_eq!(v.creator, creator.pubkey());
    assert_eq!(v.authority, creator.pubkey());
    assert_eq!(env.vault_sol(&vault), 0); // derived from vault_sol, no field
    assert_eq!(v.linked_wallets, 1);

    env.deposit_vault(&creator, &vault, LAMPORTS_PER_SOL)
        .expect("deposit");
    assert_eq!(env.vault_sol(&vault), LAMPORTS_PER_SOL);

    env.withdraw_vault(&creator, &vault, 500_000_000)
        .expect("withdraw");
    assert_eq!(env.vault_sol(&vault), 500_000_000);
}

#[test]
fn deposit_zero_amount() {
    let mut env = Env::new();
    let creator = env.new_funded(LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);
    expect_err!(
        env.deposit_vault(&creator, &vault, 0),
        TorchMarketError::ZeroAmount
    );
}

#[test]
fn withdraw_unauthorized() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);
    env.deposit_vault(&creator, &vault, LAMPORTS_PER_SOL)
        .expect("deposit");

    let other = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.withdraw_vault(&other, &vault, 100_000_000),
        TorchMarketError::VaultUnauthorized
    );
}

#[test]
fn withdraw_exceeds_balance() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);
    env.deposit_vault(&creator, &vault, LAMPORTS_PER_SOL)
        .expect("deposit");
    expect_err!(
        env.withdraw_vault(&creator, &vault, 2 * LAMPORTS_PER_SOL),
        TorchMarketError::InsufficientVaultBalance
    );
}

#[test]
fn link_unlink_lifecycle_and_mismatch() {
    let mut env = Env::new();
    let creator = env.new_funded(3 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);

    let extra = Keypair::new();
    env.link_wallet(&creator, &vault, extra.pubkey())
        .expect("link");
    assert_eq!(env.get_torch_vault(&vault.vault).linked_wallets, 2);

    // Create a SECOND vault. Try to unlink `extra` using vault2 (mismatch).
    let other_creator = env.new_funded(3 * LAMPORTS_PER_SOL);
    let vault2 = env.create_vault(&other_creator);
    expect_err!(
        env.unlink_wallet(&other_creator, &vault2, extra.pubkey()),
        TorchMarketError::VaultWalletLinkMismatch
    );

    // Proper unlink with the right vault.
    env.unlink_wallet(&creator, &vault, extra.pubkey())
        .expect("unlink");
    assert_eq!(env.get_torch_vault(&vault.vault).linked_wallets, 1);
}

#[test]
fn transfer_authority_changes_authority() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&creator);
    let new_auth = Keypair::new();

    env.transfer_vault_authority(&creator, &vault, new_auth.pubkey())
        .expect("transfer");
    assert_eq!(
        env.get_torch_vault(&vault.vault).authority,
        new_auth.pubkey()
    );

    // Original creator can no longer withdraw — authority moved.
    env.deposit_vault(&creator, &vault, LAMPORTS_PER_SOL)
        .expect("deposit (anyone)");
    expect_err!(
        env.withdraw_vault(&creator, &vault, 100_000_000),
        TorchMarketError::VaultUnauthorized
    );
}

// [F-13] TorchVault balance identity: the DERIVED balance (vault_sol lamports
// − rent) must equal total_deposited + total_received − total_withdrawn −
// total_spent after a mixed lifecycle. There is no sol_balance field — this is
// the comment-spec from architecture.md made executable.
#[test]
fn vault_derived_balance_matches_lifetime_totals() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, torch_market::constants::BONDING_TARGET_FLAME, false);
    let owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&owner);

    env.deposit_vault(&owner, &vault, 2 * LAMPORTS_PER_SOL).expect("deposit");
    env.buy_via_vault(&owner, &vault, &t, 500_000_000, 0).expect("spend (buy)");
    env.sell_via_vault(&owner, &vault, &t, 100_000_000_000, 0).expect("receive (sell)");
    env.withdraw_vault(&owner, &vault, 300_000_000).expect("withdraw");

    let v = env.get_torch_vault(&vault.vault);
    let expected = v.total_deposited + v.total_received - v.total_withdrawn - v.total_spent;
    assert_eq!(
        env.vault_sol(&vault),
        expected,
        "derived vault balance == deposit + received − withdrawn − spent"
    );
}
