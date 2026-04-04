use anchor_lang::prelude::*;

use crate::constants::{RAYDIUM_AMM_CONFIG, RAYDIUM_CPMM_PROGRAM_ID, MIN_POOL_SOL_LENDING, MAX_PRICE_DEVIATION_BPS, RATIO_PRECISION, DEPTH_TIER_1, DEPTH_TIER_2, DEPTH_TIER_3, DEPTH_LTV_0, DEPTH_LTV_1, DEPTH_LTV_2, DEPTH_LTV_3};
use crate::errors::TorchMarketError;
use crate::migration::WSOL_MINT;

// Order WSOL and token mint for Raydium (token0 < token1 by pubkey bytes).
pub fn order_mints(mint: &Pubkey) -> (Pubkey, Pubkey) {
    if WSOL_MINT < *mint {
        (WSOL_MINT, *mint)
    } else {
        (*mint, WSOL_MINT)
    }
}

// Derive the Raydium CPMM pool state PDA for a given token mint.
pub fn derive_pool_state(mint: &Pubkey) -> Pubkey {
    let (t0, t1) = order_mints(mint);
    Pubkey::find_program_address(
        &[b"pool", RAYDIUM_AMM_CONFIG.as_ref(), t0.as_ref(), t1.as_ref()],
        &RAYDIUM_CPMM_PROGRAM_ID,
    )
    .0
}

// Derive a Raydium pool vault PDA.
pub fn derive_pool_vault(pool_state: &Pubkey, vault_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"pool_vault", pool_state.as_ref(), vault_mint.as_ref()],
        &RAYDIUM_CPMM_PROGRAM_ID,
    )
    .0
}

// Derive the Raydium observation state PDA.
pub fn derive_observation_state(pool_state: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"observation", pool_state.as_ref()],
        &RAYDIUM_CPMM_PROGRAM_ID,
    )
    .0
}

// Read a token account balance from raw account data.
// TokenAccount layout: mint (32) + owner (32) + amount (8) = amount at offset 64.
pub fn read_token_account_balance(account: &AccountInfo) -> Result<u64> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 72, TorchMarketError::ZeroPoolReserves);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

// Read a Pubkey from raw account data at a given offset.
pub fn read_pubkey_at(data: &[u8], offset: usize) -> Result<Pubkey> {
    require!(data.len() >= offset + 32, TorchMarketError::ZeroPoolReserves);
    Ok(Pubkey::new_from_array(
        data[offset..offset + 32].try_into().unwrap(),
    ))
}

// Validate that pool accounts belong to the correct Raydium CPMM pool for this token.
// Reads the pool_state account data to extract stored vault and mint addresses,
// then verifies:
// 1. pool_state is owned by Raydium CPMM program
// 2. token_vault_0 and token_vault_1 match the pool's stored vaults
// 3. One pool mint is our token, the other is WSOL
// Raydium CPMM PoolState layout (after 8-byte discriminator):
//   amm_config: Pubkey (offset 8)
//   pool_creator: Pubkey (offset 40)
//   token_0_vault: Pubkey (offset 72)
//   token_1_vault: Pubkey (offset 104)
//   lp_mint: Pubkey (offset 136)
//   token_0_mint: Pubkey (offset 168)
//   token_1_mint: Pubkey (offset 200)
pub fn validate_pool_accounts(
    pool_state: &AccountInfo,
    token_vault_0: &AccountInfo,
    token_vault_1: &AccountInfo,
    expected_mint: &Pubkey,
) -> Result<()> {
    require!(
        *pool_state.owner == RAYDIUM_CPMM_PROGRAM_ID,
        TorchMarketError::InvalidPoolAccount
    );

    let data = pool_state.try_borrow_data()?;
    let stored_amm_config = read_pubkey_at(&data, 8)?;
    require!(
        stored_amm_config == crate::constants::RAYDIUM_AMM_CONFIG,
        TorchMarketError::InvalidPoolAccount
    );

    let stored_vault_0 = read_pubkey_at(&data, 72)?;
    let stored_vault_1 = read_pubkey_at(&data, 104)?;
    require!(
        token_vault_0.key() == stored_vault_0,
        TorchMarketError::InvalidPoolAccount
    );
    require!(
        token_vault_1.key() == stored_vault_1,
        TorchMarketError::InvalidPoolAccount
    );

    let mint_0 = read_pubkey_at(&data, 168)?;
    let mint_1 = read_pubkey_at(&data, 200)?;
    let has_token = mint_0 == *expected_mint || mint_1 == *expected_mint;
    let has_wsol = mint_0 == WSOL_MINT || mint_1 == WSOL_MINT;
    require!(
        has_token && has_wsol,
        TorchMarketError::InvalidPoolAccount
    );

    Ok(())
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

pub fn is_wsol_vault_0(pool_state: &AccountInfo) -> Result<bool> {
    let data = pool_state.try_borrow_data()?;
    let mint_0 = read_pubkey_at(&data, 168)?;
    Ok(mint_0 == WSOL_MINT)
}

// Read accumulated Raydium CPMM protocol + fund fees from pool state.
// Vault balances include trading reserves PLUS accumulated fees that have
// not yet been claimed. For accurate price ratio calculations, subtract
// these fees from vault balances to get actual trading reserves.
// Returns (sol_fees, token_fees).
// Raydium CPMM PoolState fee offsets (repr(C, packed), after 8-byte discriminator):
//   protocol_fees_token_0: u64 @ offset 341
//   protocol_fees_token_1: u64 @ offset 349
//   fund_fees_token_0:     u64 @ offset 357
//   fund_fees_token_1:     u64 @ offset 365
pub fn read_pool_accumulated_fees(
    pool_state: &AccountInfo,
    is_wsol_token_0: bool,
) -> Result<(u64, u64)> {
    let data = pool_state.try_borrow_data()?;
    require!(data.len() >= 373, TorchMarketError::InvalidPoolAccount);

    let protocol_fee_0 = u64::from_le_bytes(data[341..349].try_into().unwrap());
    let protocol_fee_1 = u64::from_le_bytes(data[349..357].try_into().unwrap());
    let fund_fee_0 = u64::from_le_bytes(data[357..365].try_into().unwrap());
    let fund_fee_1 = u64::from_le_bytes(data[365..373].try_into().unwrap());
    let total_fee_0 = protocol_fee_0
        .checked_add(fund_fee_0)
        .ok_or(TorchMarketError::MathOverflow)?;
    let total_fee_1 = protocol_fee_1
        .checked_add(fund_fee_1)
        .ok_or(TorchMarketError::MathOverflow)?;

    if is_wsol_token_0 {
        Ok((total_fee_0, total_fee_1)) // (sol_fees, token_fees)
    } else {
        Ok((total_fee_1, total_fee_0)) // (sol_fees, token_fees)
    }
}
