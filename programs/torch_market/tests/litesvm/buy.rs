// Buy handler tests — 10 cases covering happy paths and every reachable error.
//
// One test per error variant in TorchMarketError raised by the buy path,
// plus happy / community-token / first-buy-init / vault variant.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, signer::Signer};

use crate::{
    expect_err,
    harness::{Env, TokenCtx},
};
use torch_market::{constants::*, errors::TorchMarketError};

fn token(env: &mut Env, target: u64, community: bool) -> TokenCtx {
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.create_token(&creator, target, community)
}

// ---------------------------------------------------------------------------

#[test]
fn happy_path() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);

    env.buy(&buyer, &t, 100_000_000, 0).expect("buy");

    let bc = env.get_bonding_curve(&t);
    assert!(bc.real_sol_reserves > 0);
    assert!(bc.virtual_sol_reserves > 37_500_000_000); // > initial 37.5 SOL
    let tr = env.get_treasury(&t);
    assert!(tr.sol_balance > 0); // received treasury split
}

#[test]
fn first_buy_initializes_user_position_and_stats() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);

    let (user_position, _) = Pubkey::find_program_address(
        &[
            USER_POSITION_SEED,
            t.bonding_curve.as_ref(),
            buyer.pubkey().as_ref(),
        ],
        &torch_market::ID,
    );
    let (user_stats, _) = Pubkey::find_program_address(
        &[USER_STATS_SEED, buyer.pubkey().as_ref()],
        &torch_market::ID,
    );

    assert!(!env.account_exists(&user_position));
    assert!(!env.account_exists(&user_stats));

    env.buy(&buyer, &t, 100_000_000, 0).expect("buy");

    assert!(env.account_exists(&user_position));
    assert!(env.account_exists(&user_stats));
}

#[test]
fn max_wallet_exceeded() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(5 * LAMPORTS_PER_SOL);
    // 3 SOL early on FLAME (~37.5 SOL virtual) gives well over 20M tokens.
    expect_err!(
        env.buy(&buyer, &t, 3 * LAMPORTS_PER_SOL, 0),
        TorchMarketError::MaxWalletExceeded
    );
}

#[test]
fn slippage_exceeded() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    // 0.1 SOL gives ~2M tokens; demand 100M → impossible.
    expect_err!(
        env.buy(&buyer, &t, 100_000_000, 100_000_000_000_000),
        TorchMarketError::SlippageExceeded
    );
}

#[test]
fn insufficient_tokens_for_oversized_buy() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    // ~470 SOL on curve to demand more tokens than real_token_reserves (700M).
    // Send 600 SOL — InsufficientTokens fires before MaxWalletExceeded in the
    // handler's check order.
    let buyer = env.new_funded(700 * LAMPORTS_PER_SOL);
    expect_err!(
        env.buy(&buyer, &t, 600 * LAMPORTS_PER_SOL, 0),
        TorchMarketError::InsufficientTokens
    );
}

#[test]
fn bonding_complete_blocks_buy() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    env.bond_to_completion(&t);
    assert!(env.get_bonding_curve(&t).bonding_complete);

    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.buy(&buyer, &t, 100_000_000, 0),
        TorchMarketError::BondingComplete
    );
}

#[test]
fn amount_too_small() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    expect_err!(
        env.buy(&buyer, &t, MIN_SOL_AMOUNT - 1, 0),
        TorchMarketError::AmountTooSmall
    );
}

#[test]
fn invalid_dev_wallet() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    let bogus_dev = Pubkey::new_unique();
    expect_err!(
        env.buy_with_overrides(&buyer, &t, 100_000_000, 0, Some(bogus_dev), None),
        TorchMarketError::InvalidDevWallet
    );
}

#[test]
fn community_token_gives_creator_zero() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, true);
    // Community tokens are flagged on main by Treasury.total_bought_back = u64::MAX
    // (sentinel value, encoded for layout compat). See state.rs:120.
    assert_eq!(env.get_treasury(&t).total_bought_back, u64::MAX);

    let creator_before = env.svm.get_account(&creator.pubkey()).unwrap().lamports;
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).expect("buy");

    let creator_after = env.svm.get_account(&creator.pubkey()).unwrap().lamports;
    assert_eq!(
        creator_before, creator_after,
        "community-token creator must receive 0 SOL on buys"
    );
}

#[test]
fn buy_via_vault_happy() {
    let mut env = Env::new();
    let t = token(&mut env, BONDING_TARGET_FLAME, false);

    // Vault setup: creator creates vault, then funds it. Vault creator is
    // auto-linked, so the creator can act as the signer.
    let vault_owner = env.new_funded(5 * LAMPORTS_PER_SOL);
    let vault = env.create_vault(&vault_owner);
    env.deposit_vault(&vault_owner, &vault, 2 * LAMPORTS_PER_SOL)
        .expect("deposit");

    env.buy_via_vault(&vault_owner, &vault, &t, 100_000_000, 0)
        .expect("buy_via_vault");

    let v = env.get_torch_vault(&vault.vault);
    assert_eq!(v.sol_balance, 2 * LAMPORTS_PER_SOL - 100_000_000);
    assert_eq!(v.total_spent, 100_000_000);
}
