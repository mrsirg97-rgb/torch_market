use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;

// Contribute SOL to revive a reclaimed token.
// Anyone can contribute SOL to a reclaimed token.
// Contributors are essentially "patrons" who believe the token deserves another chance.
// They do NOT receive tokens for their contribution - once the token is revived, they can buy via the normal buy instruction.
// When cumulative contributions reach the per-tier threshold, the token is automatically revived:
// - `reclaimed` is set to false
// - `last_activity_slot` is updated to current slot
// - Normal buy/sell trading can resume
// # Arguments
// * `sol_amount` - Amount of SOL to contribute (in lamports)
// # Events
// Emits `RevivalContribution` on every contribution.
// Emits `TokenRevived` when threshold is reached.
pub fn contribute_revival(ctx: Context<ContributeRevival>, sol_amount: u64) -> Result<()> {
    require!(
        sol_amount >= MIN_SOL_AMOUNT,
        TorchMarketError::AmountTooSmall
    );

    let mint_key = ctx.accounts.bonding_curve.mint;
    let contributor_key = ctx.accounts.contributor.key();

    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.contributor.to_account_info(),
                to: ctx.accounts.bonding_curve.to_account_info(),
            },
        ),
        sol_amount,
    )?;

    let new_real_sol = ctx.accounts.bonding_curve.real_sol_reserves
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    ctx.accounts.bonding_curve.real_sol_reserves = new_real_sol;

    let bonding_target = ctx.accounts.bonding_curve.bonding_target;
    let (revival_threshold, _) = initial_virtual_reserves(bonding_target);
    let revived = new_real_sol >= revival_threshold;

    if revived {
        ctx.accounts.bonding_curve.reclaimed = false;
        ctx.accounts.bonding_curve.last_activity_slot = Clock::get()?.slot;
        emit!(TokenRevived {
            mint: mint_key,
            total_contributed: new_real_sol,
            revival_slot: ctx.accounts.bonding_curve.last_activity_slot,
        });
    }

    emit!(RevivalContribution {
        mint: mint_key,
        contributor: contributor_key,
        amount: sol_amount,
        total_contributed: new_real_sol,
        threshold: revival_threshold,
        revived,
    });

    Ok(())
}

// Emitted when someone contributes to a token revival
#[event]
pub struct RevivalContribution {
    pub mint: Pubkey,
    pub contributor: Pubkey,
    pub amount: u64,
    pub total_contributed: u64,
    pub threshold: u64,
    pub revived: bool,
}

// Emitted when a token is successfully revived
#[event]
pub struct TokenRevived {
    pub mint: Pubkey,
    pub total_contributed: u64,
    pub revival_slot: u64,
}
