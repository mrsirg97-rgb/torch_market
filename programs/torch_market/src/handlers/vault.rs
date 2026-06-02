use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::vault_physical_sol;

// Create a new Torch Vault for the signer.
// Also creates a VaultWalletLink for the creator.
pub fn create_vault(ctx: Context<CreateVault>) -> Result<()> {
    let created_at = Clock::get()?.unix_timestamp;
    {
        let vault = &mut ctx.accounts.vault;
        vault.creator = ctx.accounts.creator.key();
        vault.authority = ctx.accounts.creator.key();
        vault.total_deposited = 0;
        vault.total_withdrawn = 0;
        vault.total_spent = 0;
        vault.total_received = 0;
        vault.linked_wallets = 1;
        vault.created_at = created_at;
        vault.bump = ctx.bumps.vault;
    }
    let vault_key = ctx.accounts.vault.key();

    // Materialize the System-owned SOL home (rent-exempt, 0 data). All later vault
    // SOL flows are system_program::transfers to/from this PDA — no direct lamports.
    anchor_lang::system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.creator.to_account_info(),
                to: ctx.accounts.vault_sol.to_account_info(),
            },
        ),
        Rent::get()?.minimum_balance(0),
    )?;

    let wallet_link = &mut ctx.accounts.wallet_link;
    wallet_link.vault = vault_key;
    wallet_link.wallet = ctx.accounts.creator.key();
    wallet_link.linked_at = created_at;
    wallet_link.bump = ctx.bumps.wallet_link;

    Ok(())
}

// Deposit SOL into a vault. Anyone can deposit (multi-wallet support).
pub fn deposit_vault(ctx: Context<DepositVault>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    // SOL lands in the System-owned vault_sol (the derived balance), not the
    // program-owned TorchVault account.
    anchor_lang::system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.depositor.to_account_info(),
                to: ctx.accounts.vault_sol.to_account_info(),
            },
        ),
        sol_amount,
    )?;

    let vault = &mut ctx.accounts.vault;
    vault.total_deposited = vault
        .total_deposited
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Withdraw SOL from vault. Authority only (enforced by context has_one).
pub fn withdraw_vault(ctx: Context<WithdrawVault>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    // Derived balance (vault_sol lamports − rent), not a tracked field.
    require!(
        vault_physical_sol(&ctx.accounts.vault_sol)? >= sol_amount,
        TorchMarketError::InsufficientVaultBalance
    );

    // Seed-signed transfer out of the System-owned vault_sol (no direct lamports).
    let creator_key = ctx.accounts.vault.creator;
    let vsol_seeds: &[&[u8]] = &[
        TORCH_VAULT_SOL_SEED,
        creator_key.as_ref(),
        &[ctx.bumps.vault_sol],
    ];
    anchor_lang::system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault_sol.to_account_info(),
                to: ctx.accounts.authority.to_account_info(),
            },
            &[vsol_seeds],
        ),
        sol_amount,
    )?;

    let vault = &mut ctx.accounts.vault;
    vault.total_withdrawn = vault
        .total_withdrawn
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
    let seeds = &[TORCH_VAULT_SEED, creator_key.as_ref(), &[vault.bump]];
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
