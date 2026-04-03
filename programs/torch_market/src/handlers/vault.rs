use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;

// Create a new Torch Vault for the signer.
// Also creates a VaultWalletLink for the creator.
pub fn create_vault(ctx: Context<CreateVault>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    vault.creator = ctx.accounts.creator.key();
    vault.authority = ctx.accounts.creator.key();
    vault.sol_balance = 0;
    vault.total_deposited = 0;
    vault.total_withdrawn = 0;
    vault.total_spent = 0;
    vault.total_received = 0;
    vault.linked_wallets = 1;
    vault.created_at = Clock::get()?.unix_timestamp;
    vault.bump = ctx.bumps.vault;

    let wallet_link = &mut ctx.accounts.wallet_link;
    wallet_link.vault = vault.key();
    wallet_link.wallet = ctx.accounts.creator.key();
    wallet_link.linked_at = vault.created_at;
    wallet_link.bump = ctx.bumps.wallet_link;

    Ok(())
}

// Deposit SOL into a vault. Anyone can deposit (multi-wallet support).
pub fn deposit_vault(ctx: Context<DepositVault>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    anchor_lang::system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.depositor.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        ),
        sol_amount,
    )?;

    let vault = &mut ctx.accounts.vault;
    vault.sol_balance = vault
        .sol_balance
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_deposited = vault
        .total_deposited
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Withdraw SOL from vault. Authority only (enforced by context has_one).
pub fn withdraw_vault(ctx: Context<WithdrawVault>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    let vault = &mut ctx.accounts.vault;
    require!(
        vault.sol_balance >= sol_amount,
        TorchMarketError::InsufficientVaultBalance
    );

    vault.sol_balance = vault
        .sol_balance
        .checked_sub(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_withdrawn = vault
        .total_withdrawn
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    let vault_info = vault.to_account_info();
    let authority_info = ctx.accounts.authority.to_account_info();
    **vault_info.try_borrow_mut_lamports()? = vault_info
        .lamports()
        .checked_sub(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **authority_info.try_borrow_mut_lamports()? = authority_info
        .lamports()
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Link a new wallet to the vault. Authority only.
// Anchor's `init` constraint handles "already linked" (account already exists).
pub fn link_wallet(ctx: Context<LinkWallet>) -> Result<()> {
    let wallet_link = &mut ctx.accounts.wallet_link;
    wallet_link.vault = ctx.accounts.vault.key();
    wallet_link.wallet = ctx.accounts.wallet_to_link.key();
    wallet_link.linked_at = Clock::get()?.unix_timestamp;
    wallet_link.bump = ctx.bumps.wallet_link;
    
    let vault = &mut ctx.accounts.vault;
    vault.linked_wallets = vault
        .linked_wallets
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Unlink a wallet from the vault. Authority only.
// Closes the VaultWalletLink PDA, returning rent to authority.
pub fn unlink_wallet(ctx: Context<UnlinkWallet>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    vault.linked_wallets = vault
        .linked_wallets
        .checked_sub(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Transfer vault authority to a new wallet.
// Does NOT affect wallet links.
pub fn transfer_authority(ctx: Context<TransferVaultAuthority>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    vault.authority = ctx.accounts.new_authority.key();
    Ok(())
}

// Withdraw tokens from vault ATA to any destination.
// Authority only. Composability escape hatch for external DeFi.
pub fn withdraw_tokens(ctx: Context<WithdrawTokens>, amount: u64) -> Result<()> {
    require!(amount > 0, TorchMarketError::ZeroAmount);
    let vault = &ctx.accounts.vault;
    require!(
        ctx.accounts.vault_token_account.amount >= amount,
        TorchMarketError::InsufficientTokens
    );

    let creator_key = vault.creator;
    let seeds = &[
        TORCH_VAULT_SEED,
        creator_key.as_ref(),
        &[vault.bump],
    ];
    let signer_seeds = &[&seeds[..]][..];

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.destination_token_account.to_account_info(),
                authority: vault.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        ctx.accounts.mint.decimals,
    )?;

    Ok(())
}
