use anchor_lang::prelude::*;

use crate::constants::{
    DEEP_POOL_PROGRAM_ID, DEEP_POOL_POOL_SEED, DEEP_POOL_VAULT_SEED, DEEP_POOL_LP_MINT_SEED,
    MIN_POOL_SOL_LENDING, MAX_PRICE_DEVIATION_BPS, RATIO_PRECISION,
    DEPTH_TIER_1, DEPTH_TIER_2, DEPTH_TIER_3, DEPTH_LTV_0, DEPTH_LTV_1, DEPTH_LTV_2, DEPTH_LTV_3,
};
use crate::errors::TorchMarketError;

// Derive the DeepPool pool PDA for a given token mint.
pub fn derive_deep_pool(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[DEEP_POOL_POOL_SEED, mint.as_ref()],
        &DEEP_POOL_PROGRAM_ID,
    )
    .0
}

// Derive the DeepPool token vault PDA for a given pool.
pub fn derive_deep_pool_vault(pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[DEEP_POOL_VAULT_SEED, pool.as_ref()],
        &DEEP_POOL_PROGRAM_ID,
    )
    .0
}

// Derive the DeepPool LP mint PDA for a given pool.
pub fn derive_deep_pool_lp_mint(pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[DEEP_POOL_LP_MINT_SEED, pool.as_ref()],
        &DEEP_POOL_PROGRAM_ID,
    )
    .0
}

// Read DeepPool reserves: SOL from pool PDA lamports, tokens from vault.
// Returns (pool_sol, pool_tokens).
pub fn read_deep_pool_reserves(
    pool_info: &AccountInfo,
    token_vault: &AccountInfo,
) -> Result<(u64, u64)> {
    let rent = Rent::get()?;
    let rent_exempt = rent.minimum_balance(deep_pool::Pool::LEN);
    let pool_sol = pool_info.lamports().saturating_sub(rent_exempt);
    let pool_tokens = read_token_account_balance(token_vault)?;
    Ok((pool_sol, pool_tokens))
}

// Read a token account balance from raw account data.
// TokenAccount layout: mint (32) + owner (32) + amount (8) = amount at offset 64.
pub fn read_token_account_balance(account: &AccountInfo) -> Result<u64> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 72, TorchMarketError::ZeroPoolReserves);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

// Require minimum pool SOL liquidity.
// Used by both new positions (borrow/short) and liquidations.
pub fn require_min_pool_liquidity(pool_sol: u64) -> Result<()> {
    require!(
        pool_sol >= MIN_POOL_SOL_LENDING,
        TorchMarketError::PoolTooThin
    );
    Ok(())
}

// Require pool price is within deviation band of migration baseline.
// Used only for new positions (borrow/short). Liquidations are exempt.
// Compares current_ratio vs baseline_ratio using RATIO_PRECISION (1e9).
// Blocks if price has moved more than MAX_PRICE_DEVIATION_BPS (50%) in either direction.
pub fn require_price_in_band(
    pool_sol: u64,
    pool_tokens: u64,
    baseline_sol: u64,
    baseline_tokens: u64,
) -> Result<()> {
    // Callers enforce treasury.baseline_initialized before calling this function.
    // Zero baseline is a hard error — no silent bypass.
    require!(baseline_sol > 0 && baseline_tokens > 0, TorchMarketError::BaselineNotInitialized);

    let current_ratio = (pool_sol as u128)
        .checked_mul(RATIO_PRECISION)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(pool_tokens as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    let baseline_ratio = (baseline_sol as u128)
        .checked_mul(RATIO_PRECISION)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(baseline_tokens as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    // Upper bound: baseline * (10000 + deviation) / 10000
    let upper = baseline_ratio
        .checked_mul(10000 + MAX_PRICE_DEVIATION_BPS as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    // Lower bound: baseline * (10000 - deviation) / 10000
    let lower = baseline_ratio
        .checked_mul(10000_u128.saturating_sub(MAX_PRICE_DEVIATION_BPS as u128))
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;

    require!(
        current_ratio >= lower && current_ratio <= upper,
        TorchMarketError::PriceDeviationTooHigh
    );

    Ok(())
}

// Depth-based risk band: returns max LTV in bps based on pool SOL depth.
// More SOL = harder to manipulate = higher LTV allowed.
// Returns 0 if pool is too thin for new margin positions.
pub fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < MIN_POOL_SOL_LENDING {
        0
    } else if pool_sol < DEPTH_TIER_1 {
        DEPTH_LTV_0
    } else if pool_sol < DEPTH_TIER_2 {
        DEPTH_LTV_1
    } else if pool_sol < DEPTH_TIER_3 {
        DEPTH_LTV_2
    } else {
        DEPTH_LTV_3
    }
}
