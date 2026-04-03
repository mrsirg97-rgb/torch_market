use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;

// Star a token to show appreciation.
//
// Users can star tokens they appreciate. When a token reaches 2000 stars,
// the accumulated star SOL is automatically sent to the creator.
//
// Each user can only star each token once. The star is recorded
// on-chain via a StarRecord PDA.
//
// Costs 0.05 SOL per star (sybil protection) - sent to token treasury.
// Auto-payout to creator when threshold (2000 stars) is reached.
pub fn star_token(ctx: Context<StarToken>) -> Result<()> {
    let token_treasury = &mut ctx.accounts.token_treasury;
    let star_record = &mut ctx.accounts.star_record;
    let current_slot = Clock::get()?.slot;
    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    if ctx.accounts.torch_vault.is_some() {
        let vault = ctx.accounts.torch_vault.as_ref().unwrap();
        require!(
            vault.sol_balance >= STAR_COST_LAMPORTS,
            TorchMarketError::InsufficientVaultBalance
        );

        let vault_info = vault.to_account_info();
        let treasury_info = token_treasury.to_account_info();
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_sub(STAR_COST_LAMPORTS)
            .ok_or(TorchMarketError::MathOverflow)?;
        **treasury_info.try_borrow_mut_lamports()? = treasury_info
            .lamports()
            .checked_add(STAR_COST_LAMPORTS)
            .ok_or(TorchMarketError::MathOverflow)?;

        let vault = ctx.accounts.torch_vault.as_mut().unwrap();
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(STAR_COST_LAMPORTS)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(STAR_COST_LAMPORTS)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.user.to_account_info(),
                    to: token_treasury.to_account_info(),
                },
            ),
            STAR_COST_LAMPORTS,
        )?;
    }

    token_treasury.star_sol_balance = token_treasury
        .star_sol_balance
        .checked_add(STAR_COST_LAMPORTS)
        .ok_or(TorchMarketError::MathOverflow)?;
    token_treasury.total_stars = token_treasury
        .total_stars
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    star_record.user = ctx.accounts.user.key();
    star_record.mint = ctx.accounts.mint.key();
    star_record.starred_at_slot = current_slot;
    star_record.bump = ctx.bumps.star_record;

    if token_treasury.total_stars >= CREATOR_REWARD_THRESHOLD && !token_treasury.creator_paid_out {
        let payout_amount = token_treasury.star_sol_balance;
        if payout_amount > 0 {
            **token_treasury.to_account_info().try_borrow_mut_lamports()? -= payout_amount;
            **ctx.accounts.creator.to_account_info().try_borrow_mut_lamports()? += payout_amount;
            token_treasury.creator_paid_out = true;
            token_treasury.star_sol_balance = 0;

        }
    }

    Ok(())
}
