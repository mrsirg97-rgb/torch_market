use anchor_lang::prelude::*;

use crate::constants::{
    BONDING_CURVE_SOL_SEED, DEEP_POOL_LP_MINT_SEED, DEEP_POOL_POOL_SEED, DEEP_POOL_PROGRAM_ID,
    DEEP_POOL_VAULT_SEED, DEPTH_FLOOR_SOL, LTV_MAX_BPS, LTV_MIN_BPS, RHO_MAX_BPS,
};
use crate::errors::TorchMarketError;

// Validate a bonding_curve_sol AccountInfo against its canonical PDA and return
// the bump (for seed-signing). Used by the heavy buy/sell contexts that pass it
// as a bare AccountInfo (manual validation to spare try_accounts stack).
pub fn validate_bonding_curve_sol(acc: &AccountInfo, mint: &Pubkey) -> Result<u8> {
    let (pda, bump) =
        Pubkey::find_program_address(&[BONDING_CURVE_SOL_SEED, mint.as_ref()], &crate::ID);
    require_keys_eq!(*acc.key, pda, TorchMarketError::InvalidPoolAccount);
    Ok(bump)
}

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

// Physical treasury SOL = the System-owned `treasury_sol_vault`'s spendable
// lamports (above its rent-exempt floor). This is the SINGLE source of truth for
// how much SOL the treasury holds — derived, never a tracked field, so it can't
// drift. The one obligation against it (total_sol_lent_to_longs) stays
// tracked on Treasury; available = this − those.
pub fn treasury_physical_sol(treasury_sol_vault: &AccountInfo) -> Result<u64> {
    let rent = Rent::get()?;
    Ok(treasury_sol_vault
        .lamports()
        .saturating_sub(rent.minimum_balance(0)))
}

// Physical TorchVault SOL = the System-owned `vault_sol` PDA's spendable lamports
// (above rent). Same derive-not-track pattern as the treasury: the vault's SOL
// balance is the vault_sol lamports, never a field — unfalsifiable, can't drift.
pub fn vault_physical_sol(vault_sol: &AccountInfo) -> Result<u64> {
    let rent = Rent::get()?;
    Ok(vault_sol.lamports().saturating_sub(rent.minimum_balance(0)))
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

// [V21] Rail 1 — depth-scaled max LTV (continuous, concave). Replaces the old
// 4-step ladder. LTV(S) = LTV_MAX − (LTV_MAX−LTV_MIN)·(S_floor/S), anchored at
// S_floor (the smallest pool we lever). Below the floor → 0 (no leverage). Pure
// integer, division-only. See docs/depth-scaled-risk-rails.md.
pub fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < DEPTH_FLOOR_SOL {
        return 0;
    }
    let span = (LTV_MAX_BPS - LTV_MIN_BPS) as u64; // 3000 bps
    // span·S_floor ≤ 3000·100e9 = 3e14, far inside u64; pool_sol ≥ floor > 0.
    let drop = span.saturating_mul(DEPTH_FLOOR_SOL) / pool_sol;
    let ltv = (LTV_MAX_BPS as u64).saturating_sub(drop);
    // pool_sol ≥ floor ⇒ drop ≤ span ⇒ ltv ∈ [LTV_MIN, LTV_MAX]; clamp is belt-and-suspenders.
    ltv.clamp(LTV_MIN_BPS as u64, LTV_MAX_BPS as u64) as u16
}

// [V21] Rail 2 — size cap: the maximum SOL-debt-value a single position may
// carry, = ρ_max of pool SOL. Keeps worst-case unwind slippage (≈ debt/pool_sol)
// depth-invariant so one flat liquidation bonus clears it on every pool. Clamped
// at open (silent, like the per-user/lock caps).
//
// [audit V21-2] `RHO_MAX_BPS < 10_000` is const-asserted (constants.rs), so the
// u128 product can't overflow and the result is ≤ pool_sol ≤ u64::MAX (the
// downcast is exact). The `try_from` + `unwrap_or(0)` is belt-and-suspenders:
// on the impossible-by-assert overflow path it denies (a 0 cap clamps the borrow
// to dust) rather than wrapping OPEN to a huge cap.
pub fn max_debt_value_for_depth(pool_sol: u64) -> u64 {
    u64::try_from((pool_sol as u128 * RHO_MAX_BPS as u128) / 10_000).unwrap_or(0)
}
