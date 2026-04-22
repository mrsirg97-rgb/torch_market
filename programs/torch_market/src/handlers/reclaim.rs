use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;

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
    let treasury_sol = ctx.accounts.token_treasury.sol_balance;
    let total_sol = curve_sol
        .checked_add(treasury_sol)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        total_sol >= MIN_RECLAIM_THRESHOLD,
        TorchMarketError::BelowReclaimThreshold
    );

    if curve_sol > 0 {
        **ctx
            .accounts
            .bonding_curve
            .to_account_info()
            .try_borrow_mut_lamports()? -= curve_sol;
        **ctx
            .accounts
            .protocol_treasury
            .to_account_info()
            .try_borrow_mut_lamports()? += curve_sol;
    }

    if treasury_sol > 0 {
        **ctx
            .accounts
            .token_treasury
            .to_account_info()
            .try_borrow_mut_lamports()? -= treasury_sol;
        **ctx
            .accounts
            .protocol_treasury
            .to_account_info()
            .try_borrow_mut_lamports()? += treasury_sol;
    }

    ctx.accounts.bonding_curve.real_sol_reserves = 0;
    ctx.accounts.bonding_curve.reclaimed = true;
    ctx.accounts.token_treasury.sol_balance = 0;
    ctx.accounts.protocol_treasury.total_fees_received = ctx
        .accounts
        .protocol_treasury
        .total_fees_received
        .checked_add(total_sol)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}
