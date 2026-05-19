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
    assert_eq!(v.sol_balance, 0);
    assert_eq!(v.linked_wallets, 1);

    env.deposit_vault(&creator, &vault, LAMPORTS_PER_SOL)
        .expect("deposit");
    assert_eq!(
        env.get_torch_vault(&vault.vault).sol_balance,
        LAMPORTS_PER_SOL
    );

    env.withdraw_vault(&creator, &vault, 500_000_000)
        .expect("withdraw");
    assert_eq!(env.get_torch_vault(&vault.vault).sol_balance, 500_000_000);
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
