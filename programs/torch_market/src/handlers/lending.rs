use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::math;
use crate::pool_validation::{get_depth_max_ltv_bps, read_deep_pool_reserves};
use crate::state::{LoanPosition, Treasury};

// ============================================================================
// Shared helpers
// ============================================================================

// Accrue interest on a loan position via the Kani-verified pure helper.
// Post-condition: `last_update_slot` always advances to current slot.
fn accrue_interest(loan: &mut Account<LoanPosition>, interest_rate_bps: u16) -> Result<()> {
    let current_slot = Clock::get()?.slot;
    let (new_accrued, new_last_slot) = math::apply_interest_accrual(
        loan.borrowed_amount,
        loan.accrued_interest,
        loan.last_update_slot,
        current_slot,
        interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    loan.accrued_interest = new_accrued;
    loan.last_update_slot = new_last_slot;
    Ok(())
}

// Check LTV against the effective max (min of depth band + per-token cap).
fn check_borrow_ltv(
    loan: &LoanPosition,
    treasury_max_ltv_bps: u16,
    pool_sol: u64,
    pool_tokens: u64,
    user_collateral: u64,
    sol_to_borrow: u64,
) -> Result<u64> {
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(treasury_max_ltv_bps);
    let collateral_value = math::calc_collateral_value(user_collateral, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let total_debt = loan
        .borrowed_amount
        .checked_add(loan.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_add(sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let new_ltv =
        math::calc_ltv_bps(total_debt, collateral_value).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        new_ltv <= effective_max_ltv as u64,
        TorchMarketError::LtvExceeded
    );
    Ok(new_ltv)
}

// Enforce utilization cap + per-user borrow cap. Returns the new total lent.
fn check_borrow_caps(
    treasury: &Treasury,
    sol_to_borrow: u64,
    loan_borrowed_before: u64,
    user_collateral: u64,
) -> Result<()> {
    let new_total_lent = treasury
        .total_sol_lent
        .checked_add(sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let short_reserved = if treasury.short_selling_enabled {
        treasury.short_collateral_reserved
    } else {
        0
    };
    let available_sol = treasury
        .sol_balance
        .checked_sub(short_reserved)
        .ok_or(TorchMarketError::MathOverflow)?;
    let max_lendable = (available_sol as u128)
        .checked_mul(treasury.lending_utilization_cap_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    require!(
        new_total_lent <= max_lendable,
        TorchMarketError::LendingCapExceeded
    );
    let user_total_borrowed = loan_borrowed_before
        .checked_add(sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let max_user_borrow = (max_lendable as u128)
        .checked_mul(user_collateral as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_mul(BORROW_SHARE_MULTIPLIER as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(TOTAL_SUPPLY as u128)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    require!(
        user_total_borrowed <= max_user_borrow,
        TorchMarketError::UserBorrowCapExceeded
    );
    Ok(())
}

// Close a program-owned account: refund its lamports to `destination`, zero
// out data, reassign to system program. Used to refund rent to borrowers/
// shorters when their loan/short position is fully settled.
pub(crate) fn close_account_to<'info>(
    account: &AccountInfo<'info>,
    destination: &AccountInfo<'info>,
) -> Result<()> {
    let rent = account.lamports();
    **destination.try_borrow_mut_lamports()? = destination
        .lamports()
        .checked_add(rent)
        .ok_or(TorchMarketError::MathOverflow)?;
    **account.try_borrow_mut_lamports()? = 0;
    account.assign(&anchor_lang::solana_program::system_program::ID);
    account.resize(0)?;
    Ok(())
}

// Direct lamport shift between PDAs/wallets. Used for treasury ↔ borrower SOL.
fn shift_lamports<'info>(
    from: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    amount: u64,
    underflow_err: TorchMarketError,
) -> Result<()> {
    **from.try_borrow_mut_lamports()? = from
        .lamports()
        .checked_sub(amount)
        .ok_or(underflow_err)?;
    **to.try_borrow_mut_lamports()? = to
        .lamports()
        .checked_add(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(())
}

// Apply borrow state mutations (loan + treasury) after the SOL/token I/O.
#[allow(clippy::too_many_arguments)]
fn finalize_borrow_state(
    loan: &mut LoanPosition,
    treasury: &mut Treasury,
    user_key: Pubkey,
    mint_key: Pubkey,
    loan_bump: u8,
    user_collateral: u64,
    sol_to_borrow: u64,
    net_deposited: u64,
) -> Result<()> {
    let is_new = loan.user == Pubkey::default();
    if is_new {
        loan.user = user_key;
        loan.mint = mint_key;
        loan.bump = loan_bump;
        loan.last_update_slot = Clock::get()?.slot;
    }
    loan.collateral_amount = user_collateral;
    loan.borrowed_amount = loan
        .borrowed_amount
        .checked_add(sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    if sol_to_borrow > 0 {
        treasury.sol_balance = treasury
            .sol_balance
            .checked_sub(sol_to_borrow)
            .ok_or(TorchMarketError::InsufficientTreasury)?;
    }
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .checked_add(sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_collateral_locked = treasury
        .total_collateral_locked
        .checked_add(net_deposited)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_new {
        treasury.active_loans = treasury
            .active_loans
            .checked_add(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    Ok(())
}

// Pay-down accounting (interest first, then principal). Returns (interest_paid, principal_paid).
fn apply_interest_first_repay(
    loan: &mut LoanPosition,
    payment: u64,
    is_full_repay: bool,
) -> Result<(u64, u64)> {
    let (interest_paid, principal_paid) = if payment <= loan.accrued_interest {
        let ip = payment;
        loan.accrued_interest = loan
            .accrued_interest
            .checked_sub(payment)
            .ok_or(TorchMarketError::MathOverflow)?;
        (ip, 0)
    } else {
        let ip = loan.accrued_interest;
        let pp = payment
            .checked_sub(loan.accrued_interest)
            .ok_or(TorchMarketError::MathOverflow)?;
        loan.accrued_interest = 0;
        loan.borrowed_amount = loan
            .borrowed_amount
            .checked_sub(pp)
            .ok_or(TorchMarketError::MathOverflow)?;
        (ip, pp)
    };
    if is_full_repay {
        loan.borrowed_amount = 0;
    }
    Ok((interest_paid, principal_paid))
}

// ============================================================================
// Borrow handlers
// ============================================================================

pub fn borrow(ctx: Context<Borrow>, args: BorrowArgs) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let net_deposited = if args.collateral_amount > 0 {
        ctx.accounts.collateral_vault.reload()?;
        let before = ctx.accounts.collateral_vault.amount;
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.borrower_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.collateral_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(),
                },
            ),
            args.collateral_amount,
            TOKEN_DECIMALS,
        )?;
        ctx.accounts.collateral_vault.reload()?;
        ctx.accounts
            .collateral_vault
            .amount
            .checked_sub(before)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        0
    };

    let user_collateral = ctx
        .accounts
        .loan_position
        .collateral_amount
        .checked_add(net_deposited)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;

    let new_ltv = check_borrow_ltv(
        &ctx.accounts.loan_position,
        ctx.accounts.treasury.max_ltv_bps,
        pool_sol,
        pool_tokens,
        user_collateral,
        args.sol_to_borrow,
    )?;

    if args.sol_to_borrow > 0 {
        check_borrow_caps(
            &ctx.accounts.treasury,
            args.sol_to_borrow,
            ctx.accounts.loan_position.borrowed_amount,
            user_collateral,
        )?;
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
            args.sol_to_borrow,
            TorchMarketError::InsufficientTreasury,
        )?;
    }

    let borrower_key = ctx.accounts.borrower.key();
    let mint_key = ctx.accounts.mint.key();
    let loan_bump = ctx.bumps.loan_position;

    finalize_borrow_state(
        &mut ctx.accounts.loan_position,
        &mut ctx.accounts.treasury,
        borrower_key,
        mint_key,
        loan_bump,
        user_collateral,
        args.sol_to_borrow,
        net_deposited,
    )?;

    emit!(LoanCreated {
        mint: mint_key,
        user: borrower_key,
        collateral_amount: user_collateral,
        borrowed_amount: ctx.accounts.loan_position.borrowed_amount,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });
    Ok(())
}

pub fn borrow_via_vault(ctx: Context<BorrowViaVault>, args: BorrowArgs) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let net_deposited = if args.collateral_amount > 0 {
        ctx.accounts.collateral_vault.reload()?;
        let before = ctx.accounts.collateral_vault.amount;
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
                    to: ctx.accounts.collateral_vault.to_account_info(),
                    authority: ctx.accounts.torch_vault.to_account_info(),
                },
                vault_signer_seeds,
            ),
            args.collateral_amount,
            TOKEN_DECIMALS,
        )?;
        ctx.accounts.collateral_vault.reload()?;
        ctx.accounts
            .collateral_vault
            .amount
            .checked_sub(before)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        0
    };

    let user_collateral = ctx
        .accounts
        .loan_position
        .collateral_amount
        .checked_add(net_deposited)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;

    let new_ltv = check_borrow_ltv(
        &ctx.accounts.loan_position,
        ctx.accounts.treasury.max_ltv_bps,
        pool_sol,
        pool_tokens,
        user_collateral,
        args.sol_to_borrow,
    )?;

    if args.sol_to_borrow > 0 {
        check_borrow_caps(
            &ctx.accounts.treasury,
            args.sol_to_borrow,
            ctx.accounts.loan_position.borrowed_amount,
            user_collateral,
        )?;
        shift_lamports(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.torch_vault.to_account_info(),
            args.sol_to_borrow,
            TorchMarketError::InsufficientTreasury,
        )?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_add(args.sol_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(args.sol_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let borrower_key = ctx.accounts.borrower.key();
    let mint_key = ctx.accounts.mint.key();
    let loan_bump = ctx.bumps.loan_position;

    finalize_borrow_state(
        &mut ctx.accounts.loan_position,
        &mut ctx.accounts.treasury,
        borrower_key,
        mint_key,
        loan_bump,
        user_collateral,
        args.sol_to_borrow,
        net_deposited,
    )?;

    emit!(LoanCreated {
        mint: mint_key,
        user: borrower_key,
        collateral_amount: user_collateral,
        borrowed_amount: ctx.accounts.loan_position.borrowed_amount,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });
    Ok(())
}

// ============================================================================
// Repay handlers
// ============================================================================

pub fn repay(ctx: Context<Repay>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let total_owed = ctx
        .accounts
        .loan_position
        .borrowed_amount
        .checked_add(ctx.accounts.loan_position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_repay = sol_amount >= total_owed;
    let actual_repay = if is_full_repay { total_owed } else { sol_amount };

    let (collateral_returned, mint_key) = if is_full_repay {
        let returned = ctx.accounts.loan_position.collateral_amount;
        let mk = ctx.accounts.mint.key();
        let treasury_bump = ctx.accounts.treasury.bump;
        let treasury_seeds = &[TREASURY_SEED, mk.as_ref(), &[treasury_bump]];
        let signer_seeds = &[&treasury_seeds[..]][..];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.borrower_token_account.to_account_info(),
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            ctx.accounts.loan_position.collateral_amount,
            TOKEN_DECIMALS,
        )?;
        ctx.accounts.loan_position.collateral_amount = 0;
        (returned, mk)
    } else {
        (0, ctx.accounts.mint.key())
    };

    // Borrower transfers SOL to treasury
    anchor_lang::system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.borrower.to_account_info(),
                to: ctx.accounts.treasury.to_account_info(),
            },
        ),
        actual_repay,
    )?;

    let (interest_paid, principal_paid) =
        apply_interest_first_repay(&mut ctx.accounts.loan_position, actual_repay, is_full_repay)?;

    let treasury = &mut ctx.accounts.treasury;
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .checked_sub(principal_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(actual_repay)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_interest_collected = treasury
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_repay {
        treasury.active_loans = treasury
            .active_loans
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.total_collateral_locked = treasury
            .total_collateral_locked
            .checked_sub(collateral_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit!(LoanRepaid {
        mint: mint_key,
        user: ctx.accounts.borrower.key(),
        sol_repaid: actual_repay,
        interest_paid,
        collateral_returned,
        fully_repaid: is_full_repay,
    });
    if is_full_repay {
        close_account_to(
            &ctx.accounts.loan_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

pub fn repay_via_vault(ctx: Context<RepayViaVault>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let total_owed = ctx
        .accounts
        .loan_position
        .borrowed_amount
        .checked_add(ctx.accounts.loan_position.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_repay = sol_amount >= total_owed;
    let actual_repay = if is_full_repay { total_owed } else { sol_amount };
    let mint_key = ctx.accounts.mint.key();

    let collateral_returned = if is_full_repay {
        let returned = ctx.accounts.loan_position.collateral_amount;
        let treasury_bump = ctx.accounts.treasury.bump;
        let treasury_seeds = &[TREASURY_SEED, mint_key.as_ref(), &[treasury_bump]];
        let signer_seeds = &[&treasury_seeds[..]][..];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            ctx.accounts.loan_position.collateral_amount,
            TOKEN_DECIMALS,
        )?;
        ctx.accounts.loan_position.collateral_amount = 0;
        returned
    } else {
        0
    };

    require!(
        ctx.accounts.torch_vault.sol_balance >= actual_repay,
        TorchMarketError::InsufficientVaultBalance
    );
    shift_lamports(
        &ctx.accounts.torch_vault.to_account_info(),
        &ctx.accounts.treasury.to_account_info(),
        actual_repay,
        TorchMarketError::MathOverflow,
    )?;
    let vault = &mut ctx.accounts.torch_vault;
    vault.sol_balance = vault
        .sol_balance
        .checked_sub(actual_repay)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_spent = vault
        .total_spent
        .checked_add(actual_repay)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (interest_paid, principal_paid) =
        apply_interest_first_repay(&mut ctx.accounts.loan_position, actual_repay, is_full_repay)?;

    let treasury = &mut ctx.accounts.treasury;
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .checked_sub(principal_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(actual_repay)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_interest_collected = treasury
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_repay {
        treasury.active_loans = treasury
            .active_loans
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.total_collateral_locked = treasury
            .total_collateral_locked
            .checked_sub(collateral_returned)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit!(LoanRepaid {
        mint: mint_key,
        user: ctx.accounts.borrower.key(),
        sol_repaid: actual_repay,
        interest_paid,
        collateral_returned,
        fully_repaid: is_full_repay,
    });
    if is_full_repay {
        close_account_to(
            &ctx.accounts.loan_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

// ============================================================================
// Liquidate — shared helpers
// ============================================================================

struct LiquidationComputed {
    actual_collateral_seized: u64,
    actual_debt_covered: u64,
    bad_debt: u64,
}

fn compute_liquidation(
    loan: &LoanPosition,
    treasury: &Treasury,
    pool_sol: u64,
    pool_tokens: u64,
) -> Result<LiquidationComputed> {
    let collateral_value = math::calc_collateral_value(loan.collateral_amount, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let total_debt = loan
        .borrowed_amount
        .checked_add(loan.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let current_ltv =
        math::calc_ltv_bps(total_debt, collateral_value).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        current_ltv > treasury.liquidation_threshold_bps as u64,
        TorchMarketError::NotLiquidatable
    );
    let max_debt_to_cover = (total_debt as u128)
        .checked_mul(treasury.liquidation_close_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let debt_to_cover = max_debt_to_cover.min(total_debt);
    let collateral_to_seize = math::calc_collateral_to_seize(
        debt_to_cover,
        treasury.liquidation_bonus_bps,
        pool_tokens,
        pool_sol,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    let actual_collateral_seized = collateral_to_seize.min(loan.collateral_amount);
    let actual_debt_covered = if collateral_to_seize > loan.collateral_amount {
        math::calc_collateral_value(actual_collateral_seized, pool_sol, pool_tokens)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        debt_to_cover
    };
    // total_debt - (covered + (total_debt - debt_to_cover)) = debt_to_cover - covered.
    // Both inner subtractions are invariant-safe: debt_to_cover <= total_debt and
    // actual_debt_covered <= debt_to_cover by construction. checked_sub for loud-fail.
    let bad_debt = total_debt
        .checked_sub(
            actual_debt_covered
                .checked_add(
                    total_debt
                        .checked_sub(debt_to_cover)
                        .ok_or(TorchMarketError::MathOverflow)?,
                )
                .ok_or(TorchMarketError::MathOverflow)?,
        )
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(LiquidationComputed {
        actual_collateral_seized,
        actual_debt_covered,
        bad_debt,
    })
}

fn apply_liquidation_loan_updates(
    loan: &mut LoanPosition,
    computed: &LiquidationComputed,
) -> Result<(u64, u64)> {
    let mut remaining_paid = computed.actual_debt_covered;
    let interest_paid = if remaining_paid <= loan.accrued_interest {
        let ip = remaining_paid;
        loan.accrued_interest = loan
            .accrued_interest
            .checked_sub(remaining_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
        remaining_paid = 0;
        ip
    } else {
        let ip = loan.accrued_interest;
        remaining_paid -= loan.accrued_interest;
        loan.accrued_interest = 0;
        loan.borrowed_amount = loan
            .borrowed_amount
            .checked_sub(remaining_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
        ip
    };
    loan.collateral_amount = loan
        .collateral_amount
        .checked_sub(computed.actual_collateral_seized)
        .ok_or(TorchMarketError::MathOverflow)?;
    if computed.bad_debt > 0 {
        loan.borrowed_amount = loan
            .borrowed_amount
            .checked_sub(computed.bad_debt)
            .ok_or(TorchMarketError::MathOverflow)?;
        loan.accrued_interest = 0;
    }
    Ok((interest_paid, remaining_paid))
}

fn apply_liquidation_treasury_updates(
    treasury: &mut Treasury,
    computed: &LiquidationComputed,
    remaining_paid: u64,
    interest_paid: u64,
    fully_liquidated: bool,
) -> Result<()> {
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .checked_sub(remaining_paid)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_sub(computed.bad_debt)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(computed.actual_debt_covered)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_interest_collected = treasury
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_collateral_locked = treasury
        .total_collateral_locked
        .checked_sub(computed.actual_collateral_seized)
        .ok_or(TorchMarketError::MathOverflow)?;
    if fully_liquidated {
        treasury.active_loans = treasury
            .active_loans
            .checked_sub(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    Ok(())
}

// ============================================================================
// Liquidate handlers
// ============================================================================

pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    // No depth gate on liquidation. New positions are gated via the depth-tier
    // LTV (see `check_borrow_ltv`); existing positions must be liquidatable
    // even when pool depth has collapsed, otherwise bad debt is stranded.

    let computed = compute_liquidation(
        &ctx.accounts.loan_position,
        &ctx.accounts.treasury,
        pool_sol,
        pool_tokens,
    )?;

    let mint_key = ctx.accounts.mint.key();
    let treasury_bump = ctx.accounts.treasury.bump;
    let treasury_seeds = &[TREASURY_SEED, mint_key.as_ref(), &[treasury_bump]];
    let signer_seeds = &[&treasury_seeds[..]][..];

    if computed.actual_collateral_seized > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.liquidator_token_account.to_account_info(),
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            computed.actual_collateral_seized,
            TOKEN_DECIMALS,
        )?;
    }

    if computed.actual_debt_covered > 0 {
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.liquidator.to_account_info(),
                    to: ctx.accounts.treasury.to_account_info(),
                },
            ),
            computed.actual_debt_covered,
        )?;
    }

    let (interest_paid, remaining_paid) =
        apply_liquidation_loan_updates(&mut ctx.accounts.loan_position, &computed)?;

    let fully_liquidated = ctx.accounts.loan_position.borrowed_amount == 0
        && ctx.accounts.loan_position.accrued_interest == 0;
    apply_liquidation_treasury_updates(
        &mut ctx.accounts.treasury,
        &computed,
        remaining_paid,
        interest_paid,
        fully_liquidated,
    )?;

    emit!(LoanLiquidated {
        mint: mint_key,
        borrower: ctx.accounts.loan_position.user,
        liquidator: ctx.accounts.liquidator.key(),
        debt_covered: computed.actual_debt_covered,
        collateral_seized: computed.actual_collateral_seized,
        bad_debt: computed.bad_debt,
    });
    if fully_liquidated {
        close_account_to(
            &ctx.accounts.loan_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

pub fn liquidate_via_vault(ctx: Context<LiquidateViaVault>) -> Result<()> {
    let interest_rate = ctx.accounts.treasury.interest_rate_bps;
    accrue_interest(&mut ctx.accounts.loan_position, interest_rate)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(
        pool_sol > 0 && pool_tokens > 0,
        TorchMarketError::ZeroPoolReserves
    );
    // No depth gate on liquidation. New positions are gated via the depth-tier
    // LTV (see `check_borrow_ltv`); existing positions must be liquidatable
    // even when pool depth has collapsed, otherwise bad debt is stranded.

    let computed = compute_liquidation(
        &ctx.accounts.loan_position,
        &ctx.accounts.treasury,
        pool_sol,
        pool_tokens,
    )?;

    let mint_key = ctx.accounts.mint.key();
    let treasury_bump = ctx.accounts.treasury.bump;
    let treasury_seeds = &[TREASURY_SEED, mint_key.as_ref(), &[treasury_bump]];
    let signer_seeds = &[&treasury_seeds[..]][..];

    if computed.actual_collateral_seized > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            computed.actual_collateral_seized,
            TOKEN_DECIMALS,
        )?;
    }

    if computed.actual_debt_covered > 0 {
        require!(
            ctx.accounts.torch_vault.sol_balance >= computed.actual_debt_covered,
            TorchMarketError::InsufficientVaultBalance
        );
        shift_lamports(
            &ctx.accounts.torch_vault.to_account_info(),
            &ctx.accounts.treasury.to_account_info(),
            computed.actual_debt_covered,
            TorchMarketError::MathOverflow,
        )?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(computed.actual_debt_covered)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(computed.actual_debt_covered)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let (interest_paid, remaining_paid) =
        apply_liquidation_loan_updates(&mut ctx.accounts.loan_position, &computed)?;

    let fully_liquidated = ctx.accounts.loan_position.borrowed_amount == 0
        && ctx.accounts.loan_position.accrued_interest == 0;
    apply_liquidation_treasury_updates(
        &mut ctx.accounts.treasury,
        &computed,
        remaining_paid,
        interest_paid,
        fully_liquidated,
    )?;

    emit!(LoanLiquidated {
        mint: mint_key,
        borrower: ctx.accounts.loan_position.user,
        liquidator: ctx.accounts.liquidator.key(),
        debt_covered: computed.actual_debt_covered,
        collateral_seized: computed.actual_collateral_seized,
        bad_debt: computed.bad_debt,
    });
    if fully_liquidated {
        close_account_to(
            &ctx.accounts.loan_position.to_account_info(),
            &ctx.accounts.borrower.to_account_info(),
        )?;
    }
    Ok(())
}

// ============================================================================
// Events
// ============================================================================

#[event]
pub struct LoanCreated {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub collateral_amount: u64,
    pub borrowed_amount: u64,
    pub ltv_bps: u16,
}

#[event]
pub struct LoanRepaid {
    pub mint: Pubkey,
    pub user: Pubkey,
    pub sol_repaid: u64,
    pub interest_paid: u64,
    pub collateral_returned: u64,
    pub fully_repaid: bool,
}

#[event]
pub struct LoanLiquidated {
    pub mint: Pubkey,
    pub borrower: Pubkey,
    pub liquidator: Pubkey,
    pub debt_covered: u64,
    pub collateral_seized: u64,
    pub bad_debt: u64,
}
