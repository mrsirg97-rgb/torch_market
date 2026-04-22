use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::VaultSwap;
use crate::errors::TorchMarketError;
// Vault-routed DeepPool swap for migrated Torch tokens.
// One instruction handles both directions:
// - Buy (SOL→Token): vault SOL → DeepPool → tokens to vault ATA
// - Sell (Token→SOL): vault ATA → DeepPool → SOL to vault
// No WSOL wrapping — DeepPool uses native SOL.
pub fn vault_swap(
    ctx: Context<VaultSwap>,
    amount_in: u64,
    minimum_amount_out: u64,
    is_buy: bool,
) -> Result<()> {
    require!(amount_in > 0, TorchMarketError::AmountTooSmall);
    require!(minimum_amount_out > 0, TorchMarketError::AmountTooSmall);

    let vault = &ctx.accounts.torch_vault;
    let creator_key = vault.creator;
    let vault_bump = vault.bump;
    let vault_seeds: &[&[u8]] = &[TORCH_VAULT_SEED, creator_key.as_ref(), &[vault_bump]];
    let vault_signer = &[vault_seeds][..];

    if is_buy {
        // Buy: vault sends SOL, receives tokens
        require!(
            ctx.accounts.torch_vault.sol_balance >= amount_in,
            TorchMarketError::InsufficientVaultBalance
        );

        // Decrement sol_balance before CPI (reverts if CPI fails)
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(amount_in)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(amount_in)
            .ok_or(TorchMarketError::MathOverflow)?;

        // Pre-deposit SOL to deep_pool pool PDA (torch_market owns vault,
        // so we can debit it; deep_pool verifies the delta)
        let vault_info = ctx.accounts.torch_vault.to_account_info();
        let pool_info = ctx.accounts.deep_pool.to_account_info();
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_sub(amount_in)
            .ok_or(TorchMarketError::MathOverflow)?;
        **pool_info.try_borrow_mut_lamports()? = pool_info
            .lamports()
            .checked_add(amount_in)
            .ok_or(TorchMarketError::MathOverflow)?;

        let swap_accounts = deep_pool::cpi::accounts::Swap {
            user: ctx.accounts.torch_vault.to_account_info(),
            pool: ctx.accounts.deep_pool.to_account_info(),
            token_mint: ctx.accounts.mint.to_account_info(),
            token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
            user_token_account: ctx.accounts.vault_token_account.to_account_info(),
            token_program: ctx.accounts.token_2022_program.to_account_info(),
            associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
        };

        deep_pool::cpi::swap(
            CpiContext::new_with_signer(
                ctx.accounts.deep_pool_program.to_account_info(),
                swap_accounts,
                vault_signer,
            ),
            deep_pool::SwapArgs {
                amount_in,
                minimum_out: minimum_amount_out,
                buy: true,
            },
        )?;

        emit!(VaultSwapExecuted {
            vault: ctx.accounts.torch_vault.key(),
            mint: ctx.accounts.mint.key(),
            signer: ctx.accounts.signer.key(),
            is_buy: true,
            amount_in,
            minimum_amount_out,
        });
    } else {
        // Sell: vault sends tokens, receives SOL
        require!(
            ctx.accounts.vault_token_account.amount >= amount_in,
            TorchMarketError::InsufficientTokens
        );

        let vault_lamports_before = ctx.accounts.torch_vault.to_account_info().lamports();

        let swap_accounts = deep_pool::cpi::accounts::Swap {
            user: ctx.accounts.torch_vault.to_account_info(),
            pool: ctx.accounts.deep_pool.to_account_info(),
            token_mint: ctx.accounts.mint.to_account_info(),
            token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
            user_token_account: ctx.accounts.vault_token_account.to_account_info(),
            token_program: ctx.accounts.token_2022_program.to_account_info(),
            associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
        };

        deep_pool::cpi::swap(
            CpiContext::new_with_signer(
                ctx.accounts.deep_pool_program.to_account_info(),
                swap_accounts,
                vault_signer,
            ),
            deep_pool::SwapArgs {
                amount_in,
                minimum_out: minimum_amount_out,
                buy: false,
            },
        )?;

        let vault_lamports_after = ctx.accounts.torch_vault.to_account_info().lamports();
        let sol_received = vault_lamports_after
            .checked_sub(vault_lamports_before)
            .ok_or(TorchMarketError::MathOverflow)?;

        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_add(sol_received)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(sol_received)
            .ok_or(TorchMarketError::MathOverflow)?;

        emit!(VaultSwapExecuted {
            vault: vault.key(),
            mint: ctx.accounts.mint.key(),
            signer: ctx.accounts.signer.key(),
            is_buy: false,
            amount_in,
            minimum_amount_out,
        });
    }

    Ok(())
}

#[event]
pub struct VaultSwapExecuted {
    pub vault: Pubkey,
    pub mint: Pubkey,
    pub signer: Pubkey,
    pub is_buy: bool,
    pub amount_in: u64,
    pub minimum_amount_out: u64,
}
