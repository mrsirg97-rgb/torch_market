use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::{read_token_account_balance, validate_pool_accounts, require_min_pool_liquidity, require_price_in_band};
use crate::state::ShortPosition;

// Calculate token debt value in lamports using Raydium pool reserves.
// value = token_debt * pool_sol / pool_tokens
fn calculate_debt_value(
    token_debt: u64,
    pool_sol: u64,
    pool_tokens: u64,
) -> Result<u64> {
    let value = (token_debt as u128)
        .checked_mul(pool_sol as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(pool_tokens as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(value as u64)
}

// Calculate LTV in basis points: (debt_value * 10000) / collateral_value
fn calculate_ltv_bps(debt_value: u64, collateral_value: u64) -> Result<u64> {
    if collateral_value == 0 {
        return Ok(u64::MAX);
    }

    let ltv = (debt_value as u128)
        .checked_mul(10000)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(collateral_value as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(ltv as u64)
}

// Accrue interest on a short position in token terms.
// interest = tokens_borrowed * rate_bps * slots_elapsed / (10000 * EPOCH_DURATION_SLOTS)
fn accrue_interest(position: &mut Account<ShortPosition>, interest_rate_bps: u16) -> Result<()> {
    if position.tokens_borrowed == 0 {
        return Ok(());
    }

    let current_slot = Clock::get()?.slot;
    let slots_elapsed = current_slot.saturating_sub(position.last_update_slot);
    if slots_elapsed == 0 {
        return Ok(());
    }

    let interest = (position.tokens_borrowed as u128)
        .checked_mul(interest_rate_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_mul(slots_elapsed as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(
            10000_u128
                .checked_mul(EPOCH_DURATION_SLOTS as u128)
                .ok_or(TorchMarketError::MathOverflow)?,
        )
        .ok_or(TorchMarketError::MathOverflow)? as u64;

    position.accrued_interest = position
        .accrued_interest
        .checked_add(interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    position.last_update_slot = current_slot;

    Ok(())
}

// Enable short selling for a specific token. Admin only.
// Creates ShortConfig PDA and sets Treasury sentinel flags.
// Zeros out the repurposed `total_burned_from_buyback` field.
pub fn enable_short_selling(ctx: Context<EnableShortSelling>) -> Result<()> {
    let treasury = &mut ctx.accounts.treasury;
    treasury.total_burned_from_buyback = 0;
    treasury.buyback_percent_bps = SHORT_ENABLED_SENTINEL;

    let short_config = &mut ctx.accounts.short_config;
    short_config.mint = ctx.accounts.mint.key();
    short_config.total_tokens_lent = 0;
    short_config.active_positions = 0;
    short_config.total_interest_collected = 0;
    short_config.bump = ctx.bumps.short_config;

    emit!(ShortSellingEnabled {
        mint: ctx.accounts.mint.key(),
        authority: ctx.accounts.authority.key(),
    });

    Ok(())
}

// Open or add to a short position: post SOL collateral, borrow tokens.
// SOL collateral goes to Treasury. Tokens come from Treasury's token account.
// Creates ShortPosition on first call. Subsequent calls can add collateral and/or borrow more.
pub fn open_short(ctx: Context<OpenShort>, args: OpenShortArgs) -> Result<()> {
    require!(
        args.sol_collateral > 0 || args.tokens_to_borrow > 0,
        TorchMarketError::EmptyBorrowRequest
    );

    if args.tokens_to_borrow > 0 {
        require!(
            args.tokens_to_borrow >= MIN_SHORT_TOKENS,
            TorchMarketError::ShortTooSmall
        );
    }

    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
        require!(
            ctx.accounts.vault_token_account.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    let treasury = &ctx.accounts.treasury;
    let position = &mut ctx.accounts.short_position;

    accrue_interest(position, treasury.interest_rate_bps)?;

    if args.sol_collateral > 0 {
        if ctx.accounts.torch_vault.is_some() {
            let vault = ctx.accounts.torch_vault.as_ref().unwrap();
            require!(
                vault.sol_balance >= args.sol_collateral,
                TorchMarketError::InsufficientVaultBalance
            );

            let vault_info = vault.to_account_info();
            let treasury_info = ctx.accounts.treasury.to_account_info();
            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_sub(args.sol_collateral)
                .ok_or(TorchMarketError::MathOverflow)?;
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_add(args.sol_collateral)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_sub(args.sol_collateral)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_spent = vault
                .total_spent
                .checked_add(args.sol_collateral)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            anchor_lang::system_program::transfer(
                CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    anchor_lang::system_program::Transfer {
                        from: ctx.accounts.shorter.to_account_info(),
                        to: ctx.accounts.treasury.to_account_info(),
                    },
                ),
                args.sol_collateral,
            )?;
        }
    }

    let user_collateral = position
        .sol_collateral
        .checked_add(args.sol_collateral)
        .ok_or(TorchMarketError::MathOverflow)?;

    validate_pool_accounts(
        &ctx.accounts.pool_state,
        &ctx.accounts.token_vault_0,
        &ctx.accounts.token_vault_1,
        &ctx.accounts.mint.key(),
    )?;

    let pool_sol = read_token_account_balance(&ctx.accounts.token_vault_0)?;
    let pool_tokens = read_token_account_balance(&ctx.accounts.token_vault_1)?;
    require!(pool_sol > 0 && pool_tokens > 0, TorchMarketError::ZeroPoolReserves);

    require_min_pool_liquidity(pool_sol)?;
    require_price_in_band(
        pool_sol,
        pool_tokens,
        treasury.baseline_sol_reserves,
        treasury.baseline_token_reserves,
    )?;

    let total_token_debt = position
        .tokens_borrowed
        .checked_add(position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_add(args.tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let debt_value = calculate_debt_value(total_token_debt, pool_sol, pool_tokens)?;
    let new_ltv = calculate_ltv_bps(debt_value, user_collateral)?;
    require!(
        new_ltv <= treasury.max_ltv_bps as u64,
        TorchMarketError::LtvExceeded
    );

    if args.tokens_to_borrow > 0 {
        let short_config = &ctx.accounts.short_config;
        let new_total_tokens_lent = short_config
            .total_tokens_lent
            .checked_add(args.tokens_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;

        ctx.accounts.treasury_lock_token_account.reload()?;
        let lock_token_balance = ctx.accounts.treasury_lock_token_account.amount;
        let max_lendable_tokens = (lock_token_balance as u128)
            .checked_mul(treasury.lending_utilization_cap_bps as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(10000)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        require!(
            new_total_tokens_lent <= max_lendable_tokens,
            TorchMarketError::ShortCapExceeded
        );

        let user_total_tokens_borrowed = position
            .tokens_borrowed
            .checked_add(args.tokens_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
        // Denominator is treasury.sol_balance (SOL) because collateral is SOL.
        // Lending uses TOTAL_SUPPLY because its collateral is tokens.
        let max_user_borrow = (max_lendable_tokens as u128)
            .checked_mul(user_collateral as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_mul(BORROW_SHARE_MULTIPLIER as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(treasury.sol_balance as u128)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        require!(
            user_total_tokens_borrowed <= max_user_borrow,
            TorchMarketError::UserShortCapExceeded
        );

        let mint_key = ctx.accounts.mint.key();
        let lock_seeds = &[
            TREASURY_LOCK_SEED,
            mint_key.as_ref(),
            &[ctx.accounts.treasury_lock.bump],
        ];
        let signer_seeds = &[&lock_seeds[..]];
        let token_destination = if ctx.accounts.vault_token_account.is_some() {
            ctx.accounts
                .vault_token_account
                .as_ref()
                .unwrap()
                .to_account_info()
        } else {
            ctx.accounts.shorter_token_account.to_account_info()
        };

        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: token_destination,
                    authority: ctx.accounts.treasury_lock.to_account_info(),
                },
                signer_seeds,
            ),
            args.tokens_to_borrow,
            TOKEN_DECIMALS,
        )?;
    }

    let is_new = position.user == Pubkey::default();
    if is_new {
        position.user = ctx.accounts.shorter.key();
        position.mint = ctx.accounts.mint.key();
        position.bump = ctx.bumps.short_position;
        position.last_update_slot = Clock::get()?.slot;
    }

    position.sol_collateral = user_collateral;
    position.tokens_borrowed = position
        .tokens_borrowed
        .checked_add(args.tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;

    let treasury = &mut ctx.accounts.treasury;
    if args.sol_collateral > 0 {
        treasury.sol_balance = treasury
            .sol_balance
            .checked_add(args.sol_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
        // Track reserved short collateral (repurposed field)
        treasury.total_burned_from_buyback = treasury
            .total_burned_from_buyback
            .checked_add(args.sol_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let short_config = &mut ctx.accounts.short_config;
    if short_config.mint == Pubkey::default() {
        short_config.mint = ctx.accounts.mint.key();
        short_config.bump = ctx.bumps.short_config;
    }
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .checked_add(args.tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_new {
        short_config.active_positions = short_config
            .active_positions
            .checked_add(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit!(ShortOpened {
        mint: ctx.accounts.mint.key(),
        user: ctx.accounts.shorter.key(),
        sol_collateral: user_collateral,
        tokens_borrowed: position.tokens_borrowed,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });

    Ok(())
}

// Close or partially repay a short position: return tokens, receive SOL collateral.
// Partial close reduces token debt (interest first, then principal). Collateral stays locked.
// Full close returns all SOL collateral and closes the position (rent reclaimed).
pub fn close_short(ctx: Context<CloseShort>, token_amount: u64) -> Result<()> {
    require!(token_amount > 0, TorchMarketError::ZeroAmount);
    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
        require!(
            ctx.accounts.vault_token_account.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    let treasury = &ctx.accounts.treasury;
    let position = &mut ctx.accounts.short_position;
    let mint_key = ctx.accounts.mint.key();

    accrue_interest(position, treasury.interest_rate_bps)?;

    let total_owed = position
        .tokens_borrowed
        .checked_add(position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_close = token_amount >= total_owed;
    let actual_return = if is_full_close { total_owed } else { token_amount };
    let token_source = if ctx.accounts.vault_token_account.is_some() {
        ctx.accounts
            .vault_token_account
            .as_ref()
            .unwrap()
            .to_account_info()
    } else {
        ctx.accounts.shorter_token_account.to_account_info()
    };

    if ctx.accounts.torch_vault.is_some() {
        let vault = ctx.accounts.torch_vault.as_ref().unwrap();
        let creator_key = vault.creator;
        let vault_seeds = &[
            TORCH_VAULT_SEED,
            creator_key.as_ref(),
            &[vault.bump],
        ];
        let vault_signer_seeds = &[&vault_seeds[..]][..];

        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: token_source,
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: vault.to_account_info(),
                },
                vault_signer_seeds,
            ),
            actual_return,
            TOKEN_DECIMALS,
        )?;
    } else {
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: token_source,
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: ctx.accounts.shorter.to_account_info(),
                },
            ),
            actual_return,
            TOKEN_DECIMALS,
        )?;
    };

    let sol_returned;
    if is_full_close {
        sol_returned = position.sol_collateral;

        if ctx.accounts.torch_vault.is_some() {
            let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
            let treasury_info = ctx.accounts.treasury.to_account_info();

            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;
            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_add(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_add(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_received = vault
                .total_received
                .checked_add(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            let treasury_info = ctx.accounts.treasury.to_account_info();
            let shorter_info = ctx.accounts.shorter.to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;
            **shorter_info.try_borrow_mut_lamports()? = shorter_info
                .lamports()
                .checked_add(sol_returned)
                .ok_or(TorchMarketError::MathOverflow)?;
        }

        position.sol_collateral = 0;
    } else {
        sol_returned = 0;
    }

    let interest_paid;
    let principal_paid;
    if actual_return <= position.accrued_interest {
        interest_paid = actual_return;
        principal_paid = 0;
        position.accrued_interest = position
            .accrued_interest
            .checked_sub(actual_return)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
        interest_paid = position.accrued_interest;
        principal_paid = actual_return
            .checked_sub(position.accrued_interest)
            .ok_or(TorchMarketError::MathOverflow)?;
        position.accrued_interest = 0;
        position.tokens_borrowed = position
            .tokens_borrowed
            .checked_sub(principal_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    if is_full_close {
        position.tokens_borrowed = 0;
    }

    if is_full_close {
        let treasury = &mut ctx.accounts.treasury;
        treasury.sol_balance = treasury
            .sol_balance
            .checked_sub(sol_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.total_burned_from_buyback = treasury
            .total_burned_from_buyback
            .saturating_sub(sol_returned);
    }

    let short_config = &mut ctx.accounts.short_config;
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .saturating_sub(principal_paid);
    short_config.total_interest_collected = short_config
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_close {
        short_config.active_positions = short_config.active_positions.saturating_sub(1);
    }

    emit!(ShortClosed {
        mint: mint_key,
        user: ctx.accounts.shorter.key(),
        tokens_returned: actual_return,
        interest_paid_tokens: interest_paid,
        sol_returned,
        fully_closed: is_full_close,
    });

    Ok(())
}

// Liquidate an underwater short position.
// When token price rises and LTV exceeds liquidation threshold (65%), anyone can call this.
// Liquidator sends tokens to cover debt, receives SOL collateral (+ bonus) from treasury.
pub fn liquidate_short(ctx: Context<LiquidateShort>) -> Result<()> {
    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
        require!(
            ctx.accounts.vault_token_account.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    let treasury = &ctx.accounts.treasury;
    let position = &mut ctx.accounts.short_position;
    let mint_key = ctx.accounts.mint.key();

    accrue_interest(position, treasury.interest_rate_bps)?;
    validate_pool_accounts(
        &ctx.accounts.pool_state,
        &ctx.accounts.token_vault_0,
        &ctx.accounts.token_vault_1,
        &ctx.accounts.mint.key(),
    )?;

    let pool_sol = read_token_account_balance(&ctx.accounts.token_vault_0)?;
    let pool_tokens = read_token_account_balance(&ctx.accounts.token_vault_1)?;
    require!(pool_sol > 0 && pool_tokens > 0, TorchMarketError::ZeroPoolReserves);

    require_min_pool_liquidity(pool_sol)?;

    let total_token_debt = position
        .tokens_borrowed
        .checked_add(position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let debt_value = calculate_debt_value(total_token_debt, pool_sol, pool_tokens)?;
    let current_ltv = calculate_ltv_bps(debt_value, position.sol_collateral)?;
    require!(
        current_ltv > treasury.liquidation_threshold_bps as u64,
        TorchMarketError::ShortNotLiquidatable
    );

    let max_tokens_to_cover = (total_token_debt as u128)
        .checked_mul(treasury.liquidation_close_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let tokens_to_cover = max_tokens_to_cover.min(total_token_debt);
    let tokens_covered_value = calculate_debt_value(tokens_to_cover, pool_sol, pool_tokens)?;
    let sol_to_seize = (tokens_covered_value as u128)
        .checked_mul((10000 + treasury.liquidation_bonus_bps as u64) as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let actual_sol_seized = sol_to_seize.min(position.sol_collateral);
    let actual_tokens_covered = if sol_to_seize > position.sol_collateral {
        (tokens_to_cover as u128)
            .checked_mul(position.sol_collateral as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(sol_to_seize as u128)
            .ok_or(TorchMarketError::MathOverflow)? as u64
    } else {
        tokens_to_cover
    };

    // Simplifies to: tokens_to_cover - actual_tokens_covered (shortfall on this call).
    let bad_debt_tokens = total_token_debt.saturating_sub(
        actual_tokens_covered
            .checked_add(total_token_debt.saturating_sub(tokens_to_cover))
            .ok_or(TorchMarketError::MathOverflow)?,
    );

    if actual_tokens_covered > 0 {
        let token_source = if ctx.accounts.vault_token_account.is_some() {
            ctx.accounts
                .vault_token_account
                .as_ref()
                .unwrap()
                .to_account_info()
        } else {
            ctx.accounts.liquidator_token_account.to_account_info()
        };

        if ctx.accounts.torch_vault.is_some() {
            let vault = ctx.accounts.torch_vault.as_ref().unwrap();
            let creator_key = vault.creator;
            let vault_seeds = &[
                TORCH_VAULT_SEED,
                creator_key.as_ref(),
                &[vault.bump],
            ];
            let vault_signer_seeds = &[&vault_seeds[..]][..];

            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: token_source,
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                        authority: vault.to_account_info(),
                    },
                    vault_signer_seeds,
                ),
                actual_tokens_covered,
                TOKEN_DECIMALS,
            )?;
        } else {
            transfer_checked(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: token_source,
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                        authority: ctx.accounts.liquidator.to_account_info(),
                    },
                ),
                actual_tokens_covered,
                TOKEN_DECIMALS,
            )?;
        }
    }

    if actual_sol_seized > 0 {
        let treasury_info = ctx.accounts.treasury.to_account_info();
        if ctx.accounts.torch_vault.is_some() {
            let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;
            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_add(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_add(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_received = vault
                .total_received
                .checked_add(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            let liquidator_info = ctx.accounts.liquidator.to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;
            **liquidator_info.try_borrow_mut_lamports()? = liquidator_info
                .lamports()
                .checked_add(actual_sol_seized)
                .ok_or(TorchMarketError::MathOverflow)?;
        }
    }

    let mut remaining_tokens_paid = actual_tokens_covered;
    let interest_paid;
    if remaining_tokens_paid <= position.accrued_interest {
        interest_paid = remaining_tokens_paid;
        position.accrued_interest = position
            .accrued_interest
            .saturating_sub(remaining_tokens_paid);
        remaining_tokens_paid = 0;
    } else {
        interest_paid = position.accrued_interest;
        remaining_tokens_paid -= position.accrued_interest;
        position.accrued_interest = 0;
        position.tokens_borrowed = position
            .tokens_borrowed
            .saturating_sub(remaining_tokens_paid);
    }

    position.sol_collateral = position
        .sol_collateral
        .saturating_sub(actual_sol_seized);
    if bad_debt_tokens > 0 {
        position.tokens_borrowed = position
            .tokens_borrowed
            .saturating_sub(bad_debt_tokens);
        position.accrued_interest = 0;
    }

    let fully_liquidated = position.tokens_borrowed == 0 && position.accrued_interest == 0;
    let treasury = &mut ctx.accounts.treasury;
    treasury.sol_balance = treasury
        .sol_balance
        .saturating_sub(actual_sol_seized);
    treasury.total_burned_from_buyback = treasury
        .total_burned_from_buyback
        .saturating_sub(actual_sol_seized);

    let short_config = &mut ctx.accounts.short_config;
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .saturating_sub(remaining_tokens_paid)
        .saturating_sub(bad_debt_tokens);
    short_config.total_interest_collected = short_config
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if fully_liquidated {
        short_config.active_positions = short_config.active_positions.saturating_sub(1);
    }

    emit!(ShortLiquidated {
        mint: mint_key,
        borrower: position.user,
        liquidator: ctx.accounts.liquidator.key(),
        tokens_covered: actual_tokens_covered,
        sol_seized: actual_sol_seized,
        bad_debt_tokens,
    });

    Ok(())
}

#[event]
pub struct ShortSellingEnabled {
    pub mint: Pubkey,
    pub authority: Pubkey,
}

#[event]
pub struct ShortOpened {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub sol_collateral: u64,
    pub tokens_borrowed: u64,
    pub ltv_bps: u16,
}

#[event]
pub struct ShortClosed {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub tokens_returned: u64,
    pub interest_paid_tokens: u64,
    pub sol_returned: u64,
    pub fully_closed: bool,
}

#[event]
pub struct ShortLiquidated {
    pub mint: Pubkey,
    pub borrower: Pubkey,
    pub liquidator: Pubkey,
    pub tokens_covered: u64,
    pub sol_seized: u64,
    pub bad_debt_tokens: u64,
}
