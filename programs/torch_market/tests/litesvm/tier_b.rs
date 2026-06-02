// Tier B regression tests, ported to V21 per-token closed leverage.
//   1. liquidation_proceeds_when_pool_thin  — a thin pool doesn't block
//      liquidating an already-underwater position (mark warmed before drain).
//   2. long_position_closes_on_full_close   — Position PDA closed, rent refunded.
//   3. short_position_closes_on_full_close  — Position PDA closed, rent refunded.

use solana_sdk::{native_token::LAMPORTS_PER_SOL, signer::Signer};

use crate::harness::{Env, TokenCtx};
use torch_market::constants::*;

// Shared setup: token + bond + migrate, treasury poked above the lending floor.
// Returns env, token ctx, and the last bonding buyer (holds tokens usable as
// long collateral / short fee headroom).
fn migrated_token() -> (Env, TokenCtx, solana_sdk::signature::Keypair) {
    let mut env = Env::new();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t = env.create_token(&creator, BONDING_TARGET_FLAME, false);
    let last_buyer = env.bond_to_completion(&t);
    let migrator = env.new_funded(2 * LAMPORTS_PER_SOL);
    env.migrate(&t, &migrator).expect("migrate");
    assert!(env.get_bonding_curve(&t).migrated);
    // V21: treasury SOL is the System-owned treasury_sol_vault's lamports (derived,
    // no tracked field). Fund it above the lending-unlock gate so longs are reachable.
    env.airdrop(&t.treasury_sol_vault, 200 * LAMPORTS_PER_SOL);
    (env, t, last_buyer)
}

// ---------------------------------------------------------------------------
// 1. liquidation_proceeds_when_pool_thin
//
// A position that's underwater at the TWAP mark must stay liquidatable even
// after pool depth collapses — bad debt can't sit stranded behind a depth gate.
// The ring is warmed at healthy depth (so the read has an anchor ≥ lookback old),
// then the price is crashed and HELD across the lookback: the keeperless mark
// tracks the held crashed price via the read's lazy head-extension, so the long
// is underwater at the mark. The thin pool (below MIN_POOL_SOL_LENDING) must not
// block the liquidation — liquidate_long has no PoolTooThin gate.
// ---------------------------------------------------------------------------

#[test]
fn liquidation_proceeds_when_pool_thin() {
    let (mut env, t, borrower) = migrated_token();
    let cranker = env.new_funded(2 * LAMPORTS_PER_SOL);

    let bal = read_borrower_token_balance(&env, &t, &borrower);
    assert!(bal > 1_000_000, "borrower has no tokens after bonding");
    let collateral = bal / 2;
    // Open a long at index 0 (collateral tokens; handler sizes the SOL borrow).
    env.open_long(&borrower, &t, 0, collateral, 1)
        .expect("open_long");

    // Warm the TWAP ring at healthy depth so the mark is readable (anchored ≥
    // lookback old).
    env.warm_twap(&cranker, &t);

    // Crash spot so the position is underwater, then drain the pool thin (one
    // poke: 4.99 SOL is both the crashed price and below MIN_POOL_SOL_LENDING).
    env.poke_pool_sol(&t, MIN_POOL_SOL_LENDING - 1);
    // HOLD the crashed price across the lookback: the keeperless mark tracks it
    // via the read's lazy head-extension (no swap needed), so the long is now
    // underwater AT THE MARK, not merely at spot. The pool stays thin throughout.
    env.warp_to_slot(env.current_slot() + LIQ_TWAP_LOOKBACK_SLOTS + 200);

    let liquidator = env.new_funded(2 * LAMPORTS_PER_SOL);
    // Liquidation must not be blocked by thin pool depth (no PoolTooThin gate on
    // liquidate_long — bad debt can't sit stranded behind a depth gate).
    env.liquidate_long(&liquidator, borrower.pubkey(), &t, 0)
        .expect("liquidation must succeed even with thin pool");
}

// ---------------------------------------------------------------------------
// 2. long_position_closes_on_full_close
//
// Full close refunds the Position PDA's rent to the borrower and frees the slot
// for re-init (manual close_account in the handler).
// ---------------------------------------------------------------------------

#[test]
fn long_position_closes_on_full_close() {
    let (mut env, t, borrower) = migrated_token();

    let bal = read_borrower_token_balance(&env, &t, &borrower);
    env.open_long(&borrower, &t, 0, bal / 2, 1).expect("open_long");

    let pos_addr = position_pda(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0);
    assert!(env.account_exists(&pos_addr), "position PDA exists while open");

    // Full close (repay_fraction_bps = 10000). Surplus tolerance 0.
    env.close_long(&borrower, &t, 0, 10_000, 0).expect("close_long");

    assert!(
        !env.account_exists(&pos_addr),
        "position PDA should be closed after full close"
    );
    assert!(
        env.get_position(&t, &borrower.pubkey(), POSITION_SIDE_LONG, 0)
            .is_none(),
        "get_position returns None after full close"
    );
}

// ---------------------------------------------------------------------------
// 3. short_position_closes_on_full_close
// ---------------------------------------------------------------------------

#[test]
fn short_position_closes_on_full_close() {
    let (mut env, t, shorter) = migrated_token();
    env.airdrop(&shorter.pubkey(), 5 * LAMPORTS_PER_SOL);

    // Open a short at index 0 with 1 SOL collateral; min_out=1 (slippage floor).
    env.open_short(&shorter, &t, 0, LAMPORTS_PER_SOL, 1)
        .expect("open_short");

    let pos_addr = position_pda(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0);
    assert!(env.account_exists(&pos_addr), "position PDA exists while open");

    // Full close: vault SOL buys back the token debt, surplus to user.
    env.close_short(&shorter, &t, 0, 10_000, 0)
        .expect("close_short");

    assert!(
        !env.account_exists(&pos_addr),
        "position PDA should be closed after full close"
    );
    assert!(
        env.get_position(&t, &shorter.pubkey(), POSITION_SIDE_SHORT, 0)
            .is_none(),
        "get_position returns None after full close"
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn position_pda(
    t: &TokenCtx,
    user: &solana_sdk::pubkey::Pubkey,
    side: u8,
    index: u32,
) -> solana_sdk::pubkey::Pubkey {
    solana_sdk::pubkey::Pubkey::find_program_address(
        &[
            POSITION_SEED,
            user.as_ref(),
            t.mint.as_ref(),
            &[side],
            &index.to_le_bytes(),
        ],
        &torch_market::ID,
    )
    .0
}

fn read_borrower_token_balance(
    env: &Env,
    t: &TokenCtx,
    borrower: &solana_sdk::signature::Keypair,
) -> u64 {
    use solana_sdk::account::ReadableAccount;
    use torch_market::token_2022_utils::get_associated_token_address_2022;
    let ata = get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
    let acct = env.svm.get_account(&ata).expect("borrower ATA missing");
    let data = acct.data();
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}
