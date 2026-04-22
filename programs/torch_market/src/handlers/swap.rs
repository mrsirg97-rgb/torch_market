use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::{invoke, invoke_signed};
use anchor_spl::token::spl_token;

use crate::constants::*;
use crate::contexts::{FundVaultWsol, VaultSwap};
use crate::errors::TorchMarketError;
use crate::pool_validation::{is_wsol_vault_0, read_token_account_balance, validate_pool_accounts};

// Fund vault WSOL ATA with lamports from vault PDA.
// Isolated instruction — direct lamport manipulation only, no CPIs.
// Must be called before vault_swap (buy) in the same transaction.
// This separation avoids the Solana runtime "sum of account balances"
// error that occurs when direct lamport modifications precede CPIs.
// IMPORTANT: Decrements sol_balance here so that repeated calls without
// a matching vault_swap cannot inflate the vault's accounting.
pub fn fund_vault_wsol(ctx: Context<FundVaultWsol>, amount: u64) -> Result<()> {
    require!(amount > 0, TorchMarketError::AmountTooSmall);

    {
        let vault = &mut ctx.accounts.torch_vault;
        require!(
            vault.sol_balance >= amount,
            TorchMarketError::InsufficientVaultBalance
        );

        vault.sol_balance = vault
            .sol_balance
            .checked_sub(amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(amount)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let vault_info = ctx.accounts.torch_vault.to_account_info();
    let wsol_info = ctx.accounts.vault_wsol_account.to_account_info();
    **vault_info.try_borrow_mut_lamports()? = vault_info
        .lamports()
        .checked_sub(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **wsol_info.try_borrow_mut_lamports()? = wsol_info
        .lamports()
        .checked_add(amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Vault-routed Raydium CPMM swap for migrated Torch tokens.
// One instruction handles both directions:
// - Buy (SOL→Token): vault SOL → WSOL → Raydium → tokens to vault ATA
// - Sell (Token→SOL): vault ATA → Raydium → WSOL → SOL to vault
// WSOL ATA is persistent: created once via create_idempotent, reused across swaps.
// Closed only on sell (to unwrap proceeds back to SOL).
pub fn vault_swap(
    ctx: Context<VaultSwap>,
    amount_in: u64,
    minimum_amount_out: u64,
    is_buy: bool,
) -> Result<()> {
    require!(amount_in > 0, TorchMarketError::AmountTooSmall);
    require!(minimum_amount_out > 0, TorchMarketError::AmountTooSmall);

    validate_pool_accounts(
        &ctx.accounts.pool_state,
        &ctx.accounts.pool_token_vault_0,
        &ctx.accounts.pool_token_vault_1,
        &ctx.accounts.mint.key(),
    )?;

    let wsol_is_vault_0 = is_wsol_vault_0(&ctx.accounts.pool_state)?;
    let vault = &ctx.accounts.torch_vault;
    let creator_key = vault.creator;
    let vault_bump = vault.bump;
    let vault_seeds: &[&[u8]] = &[TORCH_VAULT_SEED, creator_key.as_ref(), &[vault_bump]];
    let vault_signer = &[vault_seeds][..];
    if is_buy {
        invoke(
            &spl_token::instruction::sync_native(
                &ctx.accounts.token_program.key(),
                &ctx.accounts.vault_wsol_account.key(),
            )?,
            &[ctx.accounts.vault_wsol_account.to_account_info()],
        )?;

        let (input_vault, output_vault) = if wsol_is_vault_0 {
            (
                ctx.accounts.pool_token_vault_0.to_account_info(),
                ctx.accounts.pool_token_vault_1.to_account_info(),
            )
        } else {
            (
                ctx.accounts.pool_token_vault_1.to_account_info(),
                ctx.accounts.pool_token_vault_0.to_account_info(),
            )
        };

        let swap_accounts = raydium_cpmm_cpi::cpi::accounts::Swap {
            payer: ctx.accounts.torch_vault.to_account_info(),
            authority: ctx.accounts.raydium_authority.to_account_info(),
            amm_config: ctx.accounts.amm_config.to_account_info(),
            pool_state: ctx.accounts.pool_state.to_account_info(),
            input_token_account: ctx.accounts.vault_wsol_account.to_account_info(),
            output_token_account: ctx.accounts.vault_token_account.to_account_info(),
            input_vault,
            output_vault,
            input_token_program: ctx.accounts.token_program.to_account_info(),
            output_token_program: ctx.accounts.token_2022_program.to_account_info(),
            input_token_mint: ctx.accounts.wsol_mint.to_account_info(),
            output_token_mint: ctx.accounts.mint.to_account_info(),
            observation_state: ctx.accounts.observation_state.to_account_info(),
        };

        raydium_cpmm_cpi::cpi::swap_base_input(
            CpiContext::new_with_signer(
                ctx.accounts.raydium_program.to_account_info(),
                swap_accounts,
                vault_signer,
            ),
            amount_in,
            minimum_amount_out,
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
        require!(
            ctx.accounts.vault_token_account.amount >= amount_in,
            TorchMarketError::InsufficientTokens
        );

        let (input_vault, output_vault) = if wsol_is_vault_0 {
            (
                ctx.accounts.pool_token_vault_1.to_account_info(),
                ctx.accounts.pool_token_vault_0.to_account_info(),
            )
        } else {
            (
                ctx.accounts.pool_token_vault_0.to_account_info(),
                ctx.accounts.pool_token_vault_1.to_account_info(),
            )
        };

        let swap_accounts = raydium_cpmm_cpi::cpi::accounts::Swap {
            payer: ctx.accounts.torch_vault.to_account_info(),
            authority: ctx.accounts.raydium_authority.to_account_info(),
            amm_config: ctx.accounts.amm_config.to_account_info(),
            pool_state: ctx.accounts.pool_state.to_account_info(),
            input_token_account: ctx.accounts.vault_token_account.to_account_info(),
            output_token_account: ctx.accounts.vault_wsol_account.to_account_info(),
            input_vault,
            output_vault,
            input_token_program: ctx.accounts.token_2022_program.to_account_info(),
            output_token_program: ctx.accounts.token_program.to_account_info(),
            input_token_mint: ctx.accounts.mint.to_account_info(),
            output_token_mint: ctx.accounts.wsol_mint.to_account_info(),
            observation_state: ctx.accounts.observation_state.to_account_info(),
        };

        raydium_cpmm_cpi::cpi::swap_base_input(
            CpiContext::new_with_signer(
                ctx.accounts.raydium_program.to_account_info(),
                swap_accounts,
                vault_signer,
            ),
            amount_in,
            minimum_amount_out,
        )?;

        let sol_received = read_token_account_balance(&ctx.accounts.vault_wsol_account)?;

        invoke_signed(
            &spl_token::instruction::close_account(
                &ctx.accounts.token_program.key(),
                &ctx.accounts.vault_wsol_account.key(),
                &ctx.accounts.torch_vault.key(),
                &ctx.accounts.torch_vault.key(),
                &[],
            )?,
            &[
                ctx.accounts.vault_wsol_account.to_account_info(),
                ctx.accounts.torch_vault.to_account_info(),
                ctx.accounts.torch_vault.to_account_info(),
            ],
            vault_signer,
        )?;

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
