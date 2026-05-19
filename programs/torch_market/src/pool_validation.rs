use anchor_lang::prelude::*;

use crate::constants::{
    DEEP_POOL_LP_MINT_SEED, DEEP_POOL_POOL_SEED, DEEP_POOL_PROGRAM_ID, DEEP_POOL_VAULT_SEED,
    DEPTH_LTV_0, DEPTH_LTV_1, DEPTH_LTV_2, DEPTH_LTV_3, DEPTH_TIER_1, DEPTH_TIER_2, DEPTH_TIER_3,
    MIN_POOL_SOL_LENDING,
};
use crate::errors::TorchMarketError;

// Derive the DeepPool pool PDA for a given token mint + creator.
pub fn derive_deep_pool(config: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[DEEP_POOL_POOL_SEED, config.as_ref(), mint.as_ref()],
        &DEEP_POOL_PROGRAM_ID,
    )
    .0
}

// Derive Torch config PDA (used as namespace for DeepPool pools)
pub fn derive_torch_config() -> Pubkey {
    use crate::constants::TORCH_CONFIG_SEED;
    Pubkey::find_program_address(&[TORCH_CONFIG_SEED], &crate::ID).0
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

// DeepPool's Anchor `#[event_cpi]` authority PDA. Required as a CPI account on
// every deep_pool instruction since v4.x — it signs the inner emit_cpi! ix.
pub fn derive_deep_pool_event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &DEEP_POOL_PROGRAM_ID).0
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
// Defense-in-depth: assert the account is owned by SPL Token-2022 before
// trusting the byte layout. Callsites that pass non-token accounts here would
// otherwise read a u64 from arbitrary data.
pub fn read_token_account_balance(account: &AccountInfo) -> Result<u64> {
    require!(
        account.owner == &crate::token_2022_utils::TOKEN_2022_PROGRAM_ID,
        TorchMarketError::InvalidPoolAccount
    );
    let data = account.try_borrow_data()?;
    require!(data.len() >= 72, TorchMarketError::ZeroPoolReserves);
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

// Depth-based risk band: returns max LTV in bps based on pool SOL depth.
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
