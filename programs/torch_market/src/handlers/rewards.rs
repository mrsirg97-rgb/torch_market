use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::state::Treasury;

// Shared: register a star, increment counters, pay creator on threshold.
fn finalize_star<'info>(
    treasury: &mut Account<'info, Treasury>,
    star_record: &mut Account<'info, crate::state::StarRecord>,
    creator_info: &AccountInfo<'info>,
    user_key: Pubkey,
    mint_key: Pubkey,
    bump: u8,
    current_slot: u64,
) -> Result<()> {
    treasury.star_sol_balance = treasury
        .star_sol_balance
        .checked_add(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_stars = treasury
        .total_stars
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    star_record.user = user_key;
    star_record.mint = mint_key;
    star_record.starred_at_slot = current_slot;
    star_record.bump = bump;

    if treasury.total_stars >= CREATOR_REWARD_THRESHOLD && !treasury.creator_paid_out {
        let payout_amount = treasury.star_sol_balance;
        if payout_amount > 0 {
            **treasury.to_account_info().try_borrow_mut_lamports()? = treasury
                .to_account_info()
                .lamports()
                .checked_sub(payout_amount)
                .ok_or(TorchMarketError::MathOverflow)?;
            **creator_info.try_borrow_mut_lamports()? = creator_info
                .lamports()
                .checked_add(payout_amount)
                .ok_or(TorchMarketError::MathOverflow)?;
            treasury.creator_paid_out = true;
            treasury.star_sol_balance = 0;
        }
    }
    Ok(())
}

// Wallet-funded star.
pub fn star_token(ctx: Context<StarToken>) -> Result<()> {
    let current_slot = Clock::get()?.slot;
    anchor_lang::system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.user.to_account_info(),
                to: ctx.accounts.token_treasury.to_account_info(),
            },
        ),
        STAR_COST_LAMPORTS,
    )?;

    let user_key = ctx.accounts.user.key();
    let mint_key = ctx.accounts.mint.key();
    let bump = ctx.bumps.star_record;
    let creator_info = ctx.accounts.creator.to_account_info();
    finalize_star(
        &mut *ctx.accounts.token_treasury,
        &mut ctx.accounts.star_record,
        &creator_info,
        user_key,
        mint_key,
        bump,
        current_slot,
    )
}

// Vault-funded star.
pub fn star_token_via_vault(ctx: Context<StarTokenViaVault>) -> Result<()> {
    let current_slot = Clock::get()?.slot;
    require!(
        ctx.accounts.torch_vault.sol_balance >= STAR_COST_LAMPORTS,
        TorchMarketError::InsufficientVaultBalance
    );

    let vault_info = ctx.accounts.torch_vault.to_account_info();
    let treasury_info = ctx.accounts.token_treasury.to_account_info();
    **vault_info.try_borrow_mut_lamports()? = vault_info
        .lamports()
        .checked_sub(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;
    **treasury_info.try_borrow_mut_lamports()? = treasury_info
        .lamports()
        .checked_add(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;
    let vault = &mut ctx.accounts.torch_vault;
    vault.sol_balance = vault
        .sol_balance
        .checked_sub(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_spent = vault
        .total_spent
        .checked_add(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;

    let user_key = ctx.accounts.user.key();
    let mint_key = ctx.accounts.mint.key();
    let bump = ctx.bumps.star_record;
    let creator_info = ctx.accounts.creator.to_account_info();
    finalize_star(
        &mut *ctx.accounts.token_treasury,
        &mut ctx.accounts.star_record,
        &creator_info,
        user_key,
        mint_key,
        bump,
        current_slot,
    )
}
