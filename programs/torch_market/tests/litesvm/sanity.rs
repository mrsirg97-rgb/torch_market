// Sanity tests — prove the harness can do the full setup → token → buy flow.
// Anything that breaks here breaks everything downstream.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signer::Signer};

use crate::harness::Env;

#[test]
fn protocol_bootstraps() {
    let env = Env::new();
    let cfg = env.get_global_config();
    assert_eq!(cfg.authority, env.authority.pubkey());
    assert_eq!(cfg.treasury, env.treasury_wallet.pubkey());
    assert_eq!(cfg.dev_wallet, env.dev_wallet.pubkey());

    let pt = env.get_protocol_treasury();
    assert_eq!(pt.authority, env.authority.pubkey());
    assert_eq!(pt.current_epoch, 0);
}

#[test]
fn create_token_then_small_buy() {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);

    let t = env.create_token(&creator, 0, false); // 0 → default 200-SOL target

    let bc = env.get_bonding_curve(&t);
    assert_eq!(bc.creator, creator.pubkey());
    assert_eq!(bc.real_sol_reserves, 0);
    assert!(!bc.bonding_complete);
    assert!(!bc.migrated);

    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    env.buy(&buyer, &t, 100_000_000, 0).unwrap();

    let bc = env.get_bonding_curve(&t);
    assert!(bc.real_sol_reserves > 0);
    let tr = env.get_treasury(&t);
    assert!(tr.sol_balance > 0);
}
