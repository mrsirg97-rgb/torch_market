use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::treasury_physical_sol;

// Reclaim SOL from a failed token (inactive and unbonded for 7+ days).
// Anyone can call this instruction to reclaim SOL from a token that:
// - Has not completed bonding
// - Has been inactive for 7+ days (1 epoch)
// - Has not already been reclaimed
// All SOL from both the bonding curve and token treasury is transferred to the protocol treasury (merged from platform treasury).
pub fn reclaim_failed_token(ctx: Context<ReclaimFailedToken>) -> Result<()> {
    require!(
        !ctx.accounts.bonding_curve.bonding_complete,
        TorchMarketError::BondingComplete
    );

    require!(
        !ctx.accounts.bonding_curve.reclaimed,
        TorchMarketError::AlreadyReclaimed
    );

    let current_slot = Clock::get()?.slot;
    let last_activity = ctx.accounts.bonding_curve.last_activity_slot;
    let slots_since_activity = current_slot.saturating_sub(last_activity);
    require!(
        slots_since_activity >= INACTIVITY_PERIOD_SLOTS,
        TorchMarketError::TokenStillActive
    );

    let curve_sol = ctx.accounts.bonding_curve.real_sol_reserves;
    // Derived physical treasury SOL (the full reclaimable balance — no
    // reservations now that the star earmark is gone).
    let treasury_sol = treasury_physical_sol(&ctx.accounts.treasury_sol_vault)?;
    let total_sol = curve_sol
        .checked_add(treasury_sol)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        total_sol >= MIN_RECLAIM_THRESHOLD,
        TorchMarketError::BelowReclaimThreshold
    );

    // Both sources are System-owned PDAs now (treasury_sol_vault, bonding_curve_sol),
    // so each moves via a seed-signed system transfer — no direct lamports, and no
    // UnbalancedInstruction ordering hazard (two CPI credits to protocol_treasury are
    // independently reconciled).
    let mint_key = ctx.accounts.mint.key();
    if treasury_sol > 0 {
        let tsv_seeds: &[&[u8]] = &[
            TREASURY_SOL_VAULT_SEED,
            mint_key.as_ref(),
            &[ctx.bumps.treasury_sol_vault],
        ];
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.treasury_sol_vault.to_account_info(),
                    to: ctx.accounts.protocol_treasury.to_account_info(),
                },
                &[tsv_seeds],
            ),
            treasury_sol,
        )?;
    }

    if curve_sol > 0 {
        let bcsol_seeds: &[&[u8]] = &[
            BONDING_CURVE_SOL_SEED,
            mint_key.as_ref(),
            &[ctx.bumps.bonding_curve_sol],
        ];
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.bonding_curve_sol.to_account_info(),
                    to: ctx.accounts.protocol_treasury.to_account_info(),
                },
                &[bcsol_seeds],
            ),
            curve_sol,
        )?;
    }

    ctx.accounts.bonding_curve.real_sol_reserves = 0;
    ctx.accounts.bonding_curve.reclaimed = true;
    ctx.accounts.protocol_treasury.total_fees_received = ctx
        .accounts
        .protocol_treasury
        .total_fees_received
        .checked_add(total_sol)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}
