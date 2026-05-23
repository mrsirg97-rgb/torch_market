use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::math;
use crate::handlers::lending::close_account_to;
use crate::pool_validation::{get_depth_max_ltv_bps, read_deep_pool_reserves};
use crate::state::{ShortConfig, ShortPosition, Treasury};

// ============================================================================
// Shared helpers
// ============================================================================

fn accrue_interest(position: &mut Account<ShortPosition>, interest_rate_bps: u16) -> Result<()> {
    let current_slot = Clock::get()?.slot;
    let (new_accrued, new_last_slot) = math::apply_short_interest_accrual(
        position.tokens_borrowed,
        position.accrued_interest,
        position.last_update_slot,
        current_slot,
        interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    position.accrued_interest = new_accrued;
    position.last_update_slot = new_last_slot;
    Ok(())
}

fn check_short_ltv(
    position: &ShortPosition,
    treasury_max_ltv_bps: u16,
    pool_sol: u64,
    pool_tokens: u64,
    user_collateral: u64,
    tokens_to_borrow: u64,
) -> Result<u64> {
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(treasury_max_ltv_bps);
    let total_token_debt = position
        .tokens_borrowed
        .checked_add(position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let debt_value = math::calc_short_debt_value(total_token_debt, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let new_ltv =
        math::calc_ltv_bps(debt_value, user_collateral).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        new_ltv <= effective_max_ltv as u64,
        TorchMarketError::LtvExceeded
    );
    Ok(new_ltv)
}

fn check_short_caps(
    treasury: &Treasury,
    short_config: &ShortConfig,
    treasury_lock_token_balance: u64,
    tokens_to_borrow: u64,
    user_currently_borrowed: u64,
) -> Result<()> {
    let new_total_tokens_lent = short_config
        .total_tokens_lent
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let max_lendable_tokens =
        math::apply_bps(treasury_lock_token_balance, treasury.lending_utilization_cap_bps)
            .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        new_total_tokens_lent <= max_lendable_tokens,
        TorchMarketError::ShortCapExceeded
    );
    let user_total_tokens_borrowed = user_currently_borrowed
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    // Per-user short cap is decoupled from treasury size — it's a flat
    // supply-share cap: 2% of TOTAL_SUPPLY (= MAX_WALLET_TOKENS, the same
    // anti-whale constant the bonding curve uses for buys). This gives the
    // protocol a unified policy: "no wallet, by any means (buying / borrowing
    // / shorting), can hold or short more than 2% of supply."
    //
    // The global utilization check above (line ~79) bounds total outstanding
    // shorts; this check bounds per-user exposure. The two are independent
    // and serve different purposes.
    require!(
        user_total_tokens_borrowed <= MAX_WALLET_TOKENS,
        TorchMarketError::UserShortCapExceeded
    );
    Ok(())
}

fn shift_lamports<'info>(
    from: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    **from.try_borrow_mut_lamports()? = from
        .lamports()
        .checked_sub(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **to.try_borrow_mut_lamports()? = to
        .lamports()
        .checked_add(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finalize_open_short_state(
    position: &mut ShortPosition,
    treasury: &mut Treasury,
    short_config: &mut ShortConfig,
    user_key: Pubkey,
    mint_key: Pubkey,
    short_position_bump: u8,
    short_config_bump: u8,
    user_collateral: u64,
    sol_collateral_added: u64,
    tokens_to_borrow: u64,
) -> Result<()> {
    let is_new = position.user == Pubkey::default();
    if is_new {
        position.user = user_key;
        position.mint = mint_key;
        position.bump = short_position_bump;
        position.last_update_slot = Clock::get()?.slot;
    }
    position.sol_collateral = user_collateral;
    position.tokens_borrowed = position
        .tokens_borrowed
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;

    if sol_collateral_added > 0 {
        treasury.sol_balance = treasury
            .sol_balance
            .checked_add(sol_collateral_added)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.short_collateral_reserved = treasury
            .short_collateral_reserved
            .checked_add(sol_collateral_added)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    if short_config.mint == Pubkey::default() {
        short_config.mint = mint_key;
        short_config.bump = short_config_bump;
    }
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_new {
        short_config.active_positions = short_config
            .active_positions
            .checked_add(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    Ok(())
}

// Pay-down accounting for close_short (interest-first in token terms).
fn apply_interest_first_close(
    position: &mut ShortPosition,
    actual_return: u64,
    is_full_close: bool,
) -> Result<(u64, u64)> {
    let (interest_paid, principal_paid) = if actual_return <= position.accrued_interest {
        let ip = actual_return;
        position.accrued_interest = position
            .accrued_interest
            .checked_sub(actual_return)
            .ok_or(TorchMarketError::MathOverflow)?;
        (ip, 0)
    } else {
        let ip = position.accrued_interest;
        let pp = actual_return
            .checked_sub(position.accrued_interest)
            .ok_or(TorchMarketError::MathOverflow)?;
        position.accrued_interest = 0;
        position.tokens_borrowed = position
            .tokens_borrowed
            .checked_sub(pp)
            .ok_or(TorchMarketError::MathOverflow)?;
        (ip, pp)
    };
    if is_full_close {
        position.tokens_borrowed = 0;
    }
    Ok((interest_paid, principal_paid))
}

// ============================================================================
// open_short handlers
// ============================================================================

pub fn open_short(ctx: Context<OpenShort>, args: OpenShortArgs) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    if args.sol_collateral > 0 {
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

    let user_collateral = ctx
        .accounts
        .short_position
        .sol_collateral
        .checked_add(args.sol_collateral)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let new_ltv = check_short_ltv(
        &ctx.accounts.short_position,
        ctx.accounts.treasury.max_ltv_bps,
        pool_sol,
        pool_tokens,
        user_collateral,
        args.tokens_to_borrow,
    )?;

    // Token-2022 transfer-fee aware: record the NET amount the shorter
    // actually receives (and therefore owes). Mirrors the borrow path in
    // handlers/lending.rs:233-254. Without this, position.tokens_borrowed
    // would be the gross transfer amount while the shorter only ever held
    // gross × (1 - fee_bps/10000), making full close arithmetically
    // unreachable.
    let net_tokens_borrowed = if args.tokens_to_borrow > 0 {
        ctx.accounts.treasury_lock_token_account.reload()?;
        let lock_token_balance = ctx.accounts.treasury_lock_token_account.amount;
        check_short_caps(
            &ctx.accounts.treasury,
            &ctx.accounts.short_config,
            lock_token_balance,
            args.tokens_to_borrow,
            ctx.accounts.short_position.tokens_borrowed,
        )?;

        ctx.accounts.shorter_token_account.reload()?;
        let before = ctx.accounts.shorter_token_account.amount;

        let mint_key = ctx.accounts.mint.key();
        let lock_bump = ctx.accounts.treasury_lock.bump;
        let lock_seeds = &[TREASURY_LOCK_SEED, mint_key.as_ref(), &[lock_bump]];
        let signer_seeds = &[&lock_seeds[..]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.shorter_token_account.to_account_info(),
                    authority: ctx.accounts.treasury_lock.to_account_info(),
                },
                signer_seeds,
            ),
            args.tokens_to_borrow,
            TOKEN_DECIMALS,
        )?;

        ctx.accounts.shorter_token_account.reload()?;
        ctx.accounts
            .shorter_token_account
            .amount
            .checked_sub(before)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        0
    };

    let shorter_key = ctx.accounts.shorter.key();
    let mint_key = ctx.accounts.mint.key();
    let short_position_bump = ctx.bumps.short_position;
    let short_config_bump = ctx.bumps.short_config;

    finalize_open_short_state(
        &mut ctx.accounts.short_position,
        &mut ctx.accounts.treasury,
        &mut ctx.accounts.short_config,
        shorter_key,
        mint_key,
        short_position_bump,
        short_config_bump,
        user_collateral,
        args.sol_collateral,
        net_tokens_borrowed,
    )?;

    emit_cpi!(ShortOpened {
        mint: mint_key,
        user: shorter_key,
        sol_collateral: user_collateral,
        tokens_borrowed: ctx.accounts.short_position.tokens_borrowed,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });
    Ok(())
}

pub fn open_short_via_vault(
    ctx: Context<OpenShortViaVault>,
    args: OpenShortArgs,
) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    if args.sol_collateral > 0 {
        require!(
            ctx.accounts.torch_vault.sol_balance >= args.sol_collateral,
            TorchMarketError::InsufficientVaultBalance
        );
        shift_lamports(
            &ctx.accounts.torch_vault.to_account_info(),
            &ctx.accounts.treasury.to_account_info(),
            args.sol_collateral,
        )?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(args.sol_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(args.sol_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let user_collateral = ctx
        .accounts
        .short_position
        .sol_collateral
        .checked_add(args.sol_collateral)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let new_ltv = check_short_ltv(
        &ctx.accounts.short_position,
        ctx.accounts.treasury.max_ltv_bps,
        pool_sol,
        pool_tokens,
        user_collateral,
        args.tokens_to_borrow,
    )?;

    // Token-2022 transfer-fee aware: record the NET amount the vault
    // actually receives (and therefore the position owes). Mirrors the
    // pattern in open_short above.
    let net_tokens_borrowed = if args.tokens_to_borrow > 0 {
        ctx.accounts.treasury_lock_token_account.reload()?;
        let lock_token_balance = ctx.accounts.treasury_lock_token_account.amount;
        check_short_caps(
            &ctx.accounts.treasury,
            &ctx.accounts.short_config,
            lock_token_balance,
            args.tokens_to_borrow,
            ctx.accounts.short_position.tokens_borrowed,
        )?;

        ctx.accounts.vault_token_account.reload()?;
        let before = ctx.accounts.vault_token_account.amount;

        let mint_key = ctx.accounts.mint.key();
        let lock_bump = ctx.accounts.treasury_lock.bump;
        let lock_seeds = &[TREASURY_LOCK_SEED, mint_key.as_ref(), &[lock_bump]];
        let signer_seeds = &[&lock_seeds[..]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.treasury_lock.to_account_info(),
                },
                signer_seeds,
            ),
            args.tokens_to_borrow,
            TOKEN_DECIMALS,
        )?;

        ctx.accounts.vault_token_account.reload()?;
        ctx.accounts
            .vault_token_account
            .amount
            .checked_sub(before)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        0
    };

    let shorter_key = ctx.accounts.shorter.key();
    let mint_key = ctx.accounts.mint.key();
    let short_position_bump = ctx.bumps.short_position;
    let short_config_bump = ctx.bumps.short_config;

    finalize_open_short_state(
        &mut ctx.accounts.short_position,
        &mut ctx.accounts.treasury,
        &mut ctx.accounts.short_config,
        shorter_key,
        mint_key,
        short_position_bump,
        short_config_bump,
        user_collateral,
        args.sol_collateral,
        net_tokens_borrowed,
    )?;

    emit_cpi!(ShortOpened {
        mint: mint_key,
        user: shorter_key,
        sol_collateral: user_collateral,
        tokens_borrowed: ctx.accounts.short_position.tokens_borrowed,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });
    Ok(())
}

// ============================================================================
// close_short handlers
// ============================================================================

pub fn close_short(ctx: Context<CloseShort>, token_amount: u64) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    let total_owed = ctx
        .accounts
        .short_position
        .tokens_borrowed
        .checked_add(ctx.accounts.short_position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_close = token_amount >= total_owed;
    let actual_return = if is_full_close { total_owed } else { token_amount };
    // Gross-up so treasury_lock receives the FULL net amount after the
    // Token-2022 transfer fee withhold (0.07% per transfer). Without this,
    // the token pool depletes 0.14% per cycle (both borrow + return hit fees).
    // Borrower covers the ~0.07% extra on close. See v20.0.0 design note.
    let return_gross = math::gross_up_for_transfer_fee(actual_return)
        .ok_or(TorchMarketError::MathOverflow)?;
    let mint_key = ctx.accounts.mint.key();

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.shorter_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                authority: ctx.accounts.shorter.to_account_info(),
            },
        ),
        return_gross,
        TOKEN_DECIMALS,
    )?;

    let sol_returned = if is_full_close {
        let returned = ctx.accounts.short_position.sol_collateral;
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.shorter.to_account_info(),
            returned,
        )?;
        ctx.accounts.short_position.sol_collateral = 0;
        returned
    } else {
        0
    };

    let (interest_paid, principal_paid) = apply_interest_first_close(
        &mut ctx.accounts.short_position,
        actual_return,
        is_full_close,
    )?;

    if is_full_close {
        let treasury = &mut ctx.accounts.treasury;
        treasury.sol_balance = treasury
            .sol_balance
            .checked_sub(sol_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.short_collateral_reserved = treasury
            .short_collateral_reserved
            .checked_sub(sol_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    let short_config = &mut ctx.accounts.short_config;
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .checked_sub(principal_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    short_config.total_interest_collected = short_config
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_close {
        short_config.active_positions = short_config
            .active_positions
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit_cpi!(ShortClosed {
        mint: mint_key,
        user: ctx.accounts.shorter.key(),
        tokens_returned: actual_return,
        interest_paid_tokens: interest_paid,
        sol_returned,
        fully_closed: is_full_close,
    });
    if is_full_close {
        close_account_to(
            &ctx.accounts.short_position.to_account_info(),
            &ctx.accounts.shorter.to_account_info(),
        )?;
    }
    Ok(())
}

pub fn close_short_via_vault(
    ctx: Context<CloseShortViaVault>,
    token_amount: u64,
) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    let total_owed = ctx
        .accounts
        .short_position
        .tokens_borrowed
        .checked_add(ctx.accounts.short_position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_close = token_amount >= total_owed;
    let actual_return = if is_full_close { total_owed } else { token_amount };
    // See close_short comment: gross-up so treasury_lock receives the full
    // net amount post-Token-2022 transfer fee.
    let return_gross = math::gross_up_for_transfer_fee(actual_return)
        .ok_or(TorchMarketError::MathOverflow)?;
    let mint_key = ctx.accounts.mint.key();

    let creator_key = ctx.accounts.torch_vault.creator;
    let vault_bump = ctx.accounts.torch_vault.bump;
    let vault_seeds = &[TORCH_VAULT_SEED, creator_key.as_ref(), &[vault_bump]];
    let vault_signer_seeds = &[&vault_seeds[..]][..];
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                authority: ctx.accounts.torch_vault.to_account_info(),
            },
            vault_signer_seeds,
        ),
        return_gross,
        TOKEN_DECIMALS,
    )?;

    let sol_returned = if is_full_close {
        let returned = ctx.accounts.short_position.sol_collateral;
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.torch_vault.to_account_info(),
            returned,
        )?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_add(returned)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(returned)
            .ok_or(TorchMarketError::MathOverflow)?;
        ctx.accounts.short_position.sol_collateral = 0;
        returned
    } else {
        0
    };

    let (interest_paid, principal_paid) = apply_interest_first_close(
        &mut ctx.accounts.short_position,
        actual_return,
        is_full_close,
    )?;

    if is_full_close {
        let treasury = &mut ctx.accounts.treasury;
        treasury.sol_balance = treasury
            .sol_balance
            .checked_sub(sol_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.short_collateral_reserved = treasury
            .short_collateral_reserved
            .checked_sub(sol_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    let short_config = &mut ctx.accounts.short_config;
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .checked_sub(principal_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    short_config.total_interest_collected = short_config
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_close {
        short_config.active_positions = short_config
            .active_positions
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit_cpi!(ShortClosed {
        mint: mint_key,
        user: ctx.accounts.shorter.key(),
        tokens_returned: actual_return,
        interest_paid_tokens: interest_paid,
        sol_returned,
        fully_closed: is_full_close,
    });
    if is_full_close {
        close_account_to(
            &ctx.accounts.short_position.to_account_info(),
            &ctx.accounts.shorter.to_account_info(),
        )?;
    }
    Ok(())
}

// ============================================================================
// liquidate_short handlers
// ============================================================================

struct ShortLiquidationComputed {
    actual_tokens_covered: u64,
    actual_sol_seized: u64,
    bad_debt_tokens: u64,
}

fn compute_short_liquidation(
    position: &ShortPosition,
    treasury: &Treasury,
    pool_sol: u64,
    pool_tokens: u64,
) -> Result<ShortLiquidationComputed> {
    let total_token_debt = position
        .tokens_borrowed
        .checked_add(position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let debt_value = math::calc_short_debt_value(total_token_debt, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let current_ltv =
        math::calc_ltv_bps(debt_value, position.sol_collateral).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        current_ltv > treasury.liquidation_threshold_bps as u64,
        TorchMarketError::ShortNotLiquidatable
    );

    let max_tokens_to_cover = math::apply_bps(total_token_debt, treasury.liquidation_close_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    let tokens_to_cover = max_tokens_to_cover.min(total_token_debt);
    let tokens_covered_value = math::calc_short_debt_value(tokens_to_cover, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sol_to_seize =
        math::calc_short_sol_to_seize(tokens_covered_value, treasury.liquidation_bonus_bps)
            .ok_or(TorchMarketError::MathOverflow)?;
    let actual_sol_seized = sol_to_seize.min(position.sol_collateral);
    let actual_tokens_covered = if sol_to_seize > position.sol_collateral {
        math::calc_short_partial_seize_proration(tokens_to_cover, position.sol_collateral, sol_to_seize)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        tokens_to_cover
    };
    let bad_debt_tokens = math::calc_bad_debt(total_token_debt, actual_tokens_covered, tokens_to_cover)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(ShortLiquidationComputed {
        actual_tokens_covered,
        actual_sol_seized,
        bad_debt_tokens,
    })
}

fn apply_short_liquidation_position_updates(
    position: &mut ShortPosition,
    computed: &ShortLiquidationComputed,
) -> Result<u64> {
    let tokens_paid = computed.actual_tokens_covered;
    let interest_paid = if tokens_paid <= position.accrued_interest {
        position.accrued_interest = position
            .accrued_interest
            .checked_sub(tokens_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
        tokens_paid
    } else {
        let ip = position.accrued_interest;
        let principal_paid = tokens_paid - position.accrued_interest;
        position.accrued_interest = 0;
        position.tokens_borrowed = position
            .tokens_borrowed
            .checked_sub(principal_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
        ip
    };
    position.sol_collateral = position
        .sol_collateral
        .checked_sub(computed.actual_sol_seized)
        .ok_or(TorchMarketError::MathOverflow)?;
    if computed.bad_debt_tokens > 0 {
        position.tokens_borrowed = position
            .tokens_borrowed
            .checked_sub(computed.bad_debt_tokens)
            .ok_or(TorchMarketError::MathOverflow)?;
        position.accrued_interest = 0;
    }
    Ok(interest_paid)
}

#[allow(clippy::too_many_arguments)]
fn apply_short_liquidation_aggregate_updates(
    treasury: &mut Treasury,
    short_config: &mut ShortConfig,
    computed: &ShortLiquidationComputed,
    interest_paid: u64,
    fully_liquidated: bool,
) -> Result<()> {
    treasury.sol_balance = treasury
        .sol_balance
        .checked_sub(computed.actual_sol_seized)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.short_collateral_reserved = treasury
        .short_collateral_reserved
        .checked_sub(computed.actual_sol_seized)
        .ok_or(TorchMarketError::MathOverflow)?;
    short_config.total_tokens_lent = short_config
        .total_tokens_lent
        .checked_sub(computed.actual_tokens_covered)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_sub(computed.bad_debt_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    short_config.total_interest_collected = short_config
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if fully_liquidated {
        short_config.active_positions = short_config
            .active_positions
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    Ok(())
}

pub fn liquidate_short(ctx: Context<LiquidateShort>) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    // No depth gate on liquidation. See lending::liquidate for rationale.

    let computed = compute_short_liquidation(
        &ctx.accounts.short_position,
        &ctx.accounts.treasury,
        pool_sol,
        pool_tokens,
    )?;

    let mint_key = ctx.accounts.mint.key();

    if computed.actual_tokens_covered > 0 {
        // Same gross-up principle as close_short: liquidator covers the
        // transfer fee so treasury_lock receives the full debt-cover net.
        let cover_gross = math::gross_up_for_transfer_fee(computed.actual_tokens_covered)
            .ok_or(TorchMarketError::MathOverflow)?;
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.liquidator_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: ctx.accounts.liquidator.to_account_info(),
                },
            ),
            cover_gross,
            TOKEN_DECIMALS,
        )?;
    }

    if computed.actual_sol_seized > 0 {
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.liquidator.to_account_info(),
            computed.actual_sol_seized,
        )?;
    }

    let interest_paid =
        apply_short_liquidation_position_updates(&mut ctx.accounts.short_position, &computed)?;
    let fully_liquidated = ctx.accounts.short_position.tokens_borrowed == 0
        && ctx.accounts.short_position.accrued_interest == 0;
    apply_short_liquidation_aggregate_updates(
        &mut ctx.accounts.treasury,
        &mut ctx.accounts.short_config,
        &computed,
        interest_paid,
        fully_liquidated,
    )?;

    emit_cpi!(ShortLiquidated {
        mint: mint_key,
        borrower: ctx.accounts.short_position.user,
        liquidator: ctx.accounts.liquidator.key(),
        tokens_covered: computed.actual_tokens_covered,
        sol_seized: computed.actual_sol_seized,
        bad_debt_tokens: computed.bad_debt_tokens,
    });
    if fully_liquidated {
        close_account_to(
            &ctx.accounts.short_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

pub fn liquidate_short_via_vault(ctx: Context<LiquidateShortViaVault>) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.short_position, interest_rate)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    // No depth gate on liquidation. See lending::liquidate for rationale.

    let computed = compute_short_liquidation(
        &ctx.accounts.short_position,
        &ctx.accounts.treasury,
        pool_sol,
        pool_tokens,
    )?;

    let mint_key = ctx.accounts.mint.key();

    if computed.actual_tokens_covered > 0 {
        // Same gross-up principle: liquidator's vault covers fee so
        // treasury_lock receives the full debt-cover net.
        let cover_gross = math::gross_up_for_transfer_fee(computed.actual_tokens_covered)
            .ok_or(TorchMarketError::MathOverflow)?;
        let creator_key = ctx.accounts.torch_vault.creator;
        let vault_bump = ctx.accounts.torch_vault.bump;
        let vault_seeds = &[TORCH_VAULT_SEED, creator_key.as_ref(), &[vault_bump]];
        let vault_signer_seeds = &[&vault_seeds[..]][..];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: ctx.accounts.torch_vault.to_account_info(),
                },
                vault_signer_seeds,
            ),
            cover_gross,
            TOKEN_DECIMALS,
        )?;
    }

    if computed.actual_sol_seized > 0 {
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.torch_vault.to_account_info(),
            computed.actual_sol_seized,
        )?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_add(computed.actual_sol_seized)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(computed.actual_sol_seized)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let interest_paid =
        apply_short_liquidation_position_updates(&mut ctx.accounts.short_position, &computed)?;
    let fully_liquidated = ctx.accounts.short_position.tokens_borrowed == 0
        && ctx.accounts.short_position.accrued_interest == 0;
    apply_short_liquidation_aggregate_updates(
        &mut ctx.accounts.treasury,
        &mut ctx.accounts.short_config,
        &computed,
        interest_paid,
        fully_liquidated,
    )?;

    emit_cpi!(ShortLiquidated {
        mint: mint_key,
        borrower: ctx.accounts.short_position.user,
        liquidator: ctx.accounts.liquidator.key(),
        tokens_covered: computed.actual_tokens_covered,
        sol_seized: computed.actual_sol_seized,
        bad_debt_tokens: computed.bad_debt_tokens,
    });
    if fully_liquidated {
        close_account_to(
            &ctx.accounts.short_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

// ============================================================================
// Events
// ============================================================================

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
