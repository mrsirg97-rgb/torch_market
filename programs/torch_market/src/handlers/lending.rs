use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::{read_deep_pool_reserves, require_min_pool_liquidity, get_depth_max_ltv_bps};
use crate::state::LoanPosition;

// value = collateral_amount * pool_sol_reserves / pool_token_reserves
fn calculate_collateral_value(
    collateral_amount: u64,
    pool_sol: u64,
    pool_tokens: u64,
) -> Result<u64> {
    let value = (collateral_amount as u128)
        .checked_mul(pool_sol as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(pool_tokens as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(value as u64)
}

// Calculate LTV in basis points: (debt * 10000) / collateral_value
// Returns u64 to prevent silent truncation at extreme values.
fn calculate_ltv_bps(debt: u64, collateral_value: u64) -> Result<u64> {
    if collateral_value == 0 {
        return Ok(u64::MAX);
    }

    let ltv = (debt as u128)
        .checked_mul(10000)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(collateral_value as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(ltv as u64)
}

// Accrue interest on a loan position. Updates accrued_interest and last_update_slot.
// interest = principal * rate_bps * slots_elapsed / (10000 * EPOCH_DURATION_SLOTS)
fn accrue_interest(loan: &mut Account<LoanPosition>, interest_rate_bps: u16) -> Result<()> {
    if loan.borrowed_amount == 0 {
        return Ok(());
    }

    let current_slot = Clock::get()?.slot;
    let slots_elapsed = current_slot.saturating_sub(loan.last_update_slot);
    if slots_elapsed == 0 {
        return Ok(());
    }

    let interest = (loan.borrowed_amount as u128)
        .checked_mul(interest_rate_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_mul(slots_elapsed as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000_u128.checked_mul(EPOCH_DURATION_SLOTS as u128).ok_or(TorchMarketError::MathOverflow)?)
        .ok_or(TorchMarketError::MathOverflow)? as u64;

    loan.accrued_interest = loan
        .accrued_interest
        .checked_add(interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    loan.last_update_slot = current_slot;

    Ok(())
}

// Borrow SOL from treasury using tokens as collateral.
// Lock tokens in collateral vault, receive SOL up to max_ltv_bps of collateral value.
// Creates LoanPosition on first call. Subsequent calls can add collateral and/or borrow more.
pub fn borrow(ctx: Context<Borrow>, args: BorrowArgs) -> Result<()> {
    // Must provide at least one of collateral or borrow amount
    require!(
        args.collateral_amount > 0 || args.sol_to_borrow > 0,
        TorchMarketError::EmptyBorrowRequest
    );

    if args.sol_to_borrow > 0 {
        require!(
            args.sol_to_borrow >= MIN_BORROW_AMOUNT,
            TorchMarketError::BorrowTooSmall
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
    let loan = &mut ctx.accounts.loan_position;
    let interest_rate = treasury.interest_rate_bps;
    accrue_interest(loan, interest_rate)?;

    let vault_balance_before = if args.collateral_amount > 0 {
        ctx.accounts.collateral_vault.reload()?;
        ctx.accounts.collateral_vault.amount
    } else {
        0
    };

    if args.collateral_amount > 0 {
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
                        from: ctx.accounts.vault_token_account.as_ref().unwrap().to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.collateral_vault.to_account_info(),
                        authority: vault.to_account_info(),
                    },
                    vault_signer_seeds,
                ),
                args.collateral_amount,
                TOKEN_DECIMALS,
            )?;
        } else {
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
        }
    }

    let net_deposited = if args.collateral_amount > 0 {
        ctx.accounts.collateral_vault.reload()?;
        ctx.accounts.collateral_vault.amount
            .checked_sub(vault_balance_before)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        0
    };

    let user_collateral = loan
        .collateral_amount
        .checked_add(net_deposited)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(pool_sol > 0 && pool_tokens > 0, TorchMarketError::ZeroPoolReserves);

    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(treasury.max_ltv_bps);

    let collateral_value = calculate_collateral_value(user_collateral, pool_sol, pool_tokens)?;
    let total_debt = loan
        .borrowed_amount
        .checked_add(loan.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_add(args.sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    let new_ltv = calculate_ltv_bps(total_debt, collateral_value)?;
    require!(
        new_ltv <= effective_max_ltv as u64,
        TorchMarketError::LtvExceeded
    );

    if args.sol_to_borrow > 0 {
        let new_total_lent = treasury
            .total_sol_lent
            .checked_add(args.sol_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
        let short_reserved = if treasury.buyback_percent_bps == SHORT_ENABLED_SENTINEL {
            treasury.total_burned_from_buyback // repurposed: total_short_sol_collateral
        } else {
            0
        };

        let available_sol = treasury.sol_balance.saturating_sub(short_reserved);
        let max_lendable = (available_sol as u128)
            .checked_mul(treasury.lending_utilization_cap_bps as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(10000)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        require!(
            new_total_lent <= max_lendable,
            TorchMarketError::LendingCapExceeded
        );

        let user_total_borrowed = loan
            .borrowed_amount
            .checked_add(args.sol_to_borrow)
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

        let treasury_info = ctx.accounts.treasury.to_account_info();
        if ctx.accounts.torch_vault.is_some() {
            let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(args.sol_to_borrow)
                .ok_or(TorchMarketError::InsufficientTreasury)?;
            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_add(args.sol_to_borrow)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_add(args.sol_to_borrow)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_received = vault
                .total_received
                .checked_add(args.sol_to_borrow)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            let borrower_info = ctx.accounts.borrower.to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(args.sol_to_borrow)
                .ok_or(TorchMarketError::InsufficientTreasury)?;
            **borrower_info.try_borrow_mut_lamports()? = borrower_info
                .lamports()
                .checked_add(args.sol_to_borrow)
                .ok_or(TorchMarketError::MathOverflow)?;
        }
    }

    let is_new = loan.user == Pubkey::default();
    if is_new {
        loan.user = ctx.accounts.borrower.key();
        loan.mint = ctx.accounts.mint.key();
        loan.bump = ctx.bumps.loan_position;
        loan.last_update_slot = Clock::get()?.slot;
    }

    loan.collateral_amount = user_collateral;
    loan.borrowed_amount = loan
        .borrowed_amount
        .checked_add(args.sol_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;

    let treasury = &mut ctx.accounts.treasury;
    if args.sol_to_borrow > 0 {
        treasury.sol_balance = treasury
            .sol_balance
            .checked_sub(args.sol_to_borrow)
            .ok_or(TorchMarketError::InsufficientTreasury)?;
    }
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .checked_add(args.sol_to_borrow)
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

    emit!(LoanCreated {
        mint: ctx.accounts.mint.key(),
        user: ctx.accounts.borrower.key(),
        collateral_amount: user_collateral,
        borrowed_amount: loan.borrowed_amount,
        ltv_bps: new_ltv.min(u16::MAX as u64) as u16,
    });

    Ok(())
}

// Repay SOL debt and receive collateral back.
// Partial repay reduces debt (interest first, then principal). Collateral stays locked.
// Full repay returns all collateral and closes the position (rent reclaimed).
pub fn repay(ctx: Context<Repay>, sol_amount: u64) -> Result<()> {
    require!(sol_amount > 0, TorchMarketError::ZeroAmount);
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
    let loan = &mut ctx.accounts.loan_position;
    let mint_key = ctx.accounts.mint.key();
    accrue_interest(loan, treasury.interest_rate_bps)?;

    let total_owed = loan
        .borrowed_amount
        .checked_add(loan.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_full_repay = sol_amount >= total_owed;
    let actual_repay = if is_full_repay { total_owed } else { sol_amount };
    let collateral_returned;
    if is_full_repay {
        collateral_returned = loan.collateral_amount;
        let treasury_seeds = &[
            TREASURY_SEED,
            mint_key.as_ref(),
            &[treasury.bump],
        ];
        let signer_seeds = &[&treasury_seeds[..]];
        let collateral_destination = if ctx.accounts.vault_token_account.is_some() {
            ctx.accounts.vault_token_account.as_ref().unwrap().to_account_info()
        } else {
            ctx.accounts.borrower_token_account.to_account_info()
        };

        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: collateral_destination,
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            loan.collateral_amount,
            TOKEN_DECIMALS,
        )?;

        loan.collateral_amount = 0;
    } else {
        collateral_returned = 0;
    }

    if ctx.accounts.torch_vault.is_some() {
        let vault = ctx.accounts.torch_vault.as_ref().unwrap();
        require!(
            vault.sol_balance >= actual_repay,
            TorchMarketError::InsufficientVaultBalance
        );

        let vault_info = vault.to_account_info();
        let treasury_info = ctx.accounts.treasury.to_account_info();
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_sub(actual_repay)
            .ok_or(TorchMarketError::MathOverflow)?;
        **treasury_info.try_borrow_mut_lamports()? = treasury_info
            .lamports()
            .checked_add(actual_repay)
            .ok_or(TorchMarketError::MathOverflow)?;

        let vault = ctx.accounts.torch_vault.as_mut().unwrap();
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(actual_repay)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(actual_repay)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
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
    }

    let interest_paid;
    let principal_paid;
    if actual_repay <= loan.accrued_interest {
        interest_paid = actual_repay;
        principal_paid = 0;
        loan.accrued_interest = loan
            .accrued_interest
            .checked_sub(actual_repay)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
        interest_paid = loan.accrued_interest;
        principal_paid = actual_repay
            .checked_sub(loan.accrued_interest)
            .ok_or(TorchMarketError::MathOverflow)?;
        loan.accrued_interest = 0;
        loan.borrowed_amount = loan
            .borrowed_amount
            .checked_sub(principal_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    if is_full_repay {
        loan.borrowed_amount = 0;
    }

    let treasury = &mut ctx.accounts.treasury;
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .saturating_sub(principal_paid);
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(actual_repay)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_interest_collected = treasury
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    if is_full_repay {
        treasury.active_loans = treasury.active_loans.saturating_sub(1);
        treasury.total_collateral_locked = treasury
            .total_collateral_locked
            .saturating_sub(collateral_returned);
    }

    emit!(LoanRepaid {
        mint: mint_key,
        user: ctx.accounts.borrower.key(),
        sol_repaid: actual_repay,
        interest_paid,
        collateral_returned,
        fully_repaid: is_full_repay,
    });

    Ok(())
}

// Liquidate an underwater loan position.
// When LTV exceeds liquidation_threshold_bps, anyone can call this.
// Liquidator pays SOL to treasury (covering part of debt), receives collateral tokens worth (debt_covered + bonus).
pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {
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
    let loan = &mut ctx.accounts.loan_position;
    let mint_key = ctx.accounts.mint.key();

    accrue_interest(loan, treasury.interest_rate_bps)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    require!(pool_sol > 0 && pool_tokens > 0, TorchMarketError::ZeroPoolReserves);

    require_min_pool_liquidity(pool_sol)?;

    let collateral_value = calculate_collateral_value(
        loan.collateral_amount,
        pool_sol,
        pool_tokens,
    )?;

    let total_debt = loan
        .borrowed_amount
        .checked_add(loan.accrued_interest)
        .ok_or(TorchMarketError::MathOverflow)?;
    let current_ltv = calculate_ltv_bps(total_debt, collateral_value)?;
    require!(
        current_ltv > treasury.liquidation_threshold_bps as u64,
        TorchMarketError::NotLiquidatable
    );

    let max_debt_to_cover = (total_debt as u128)
        .checked_mul(treasury.liquidation_close_bps as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let debt_to_cover = max_debt_to_cover.min(total_debt);
    // Collateral to seize: debt_covered * (1 + bonus) / token_price
    // = debt_covered * (10000 + bonus_bps) / 10000 * pool_tokens / pool_sol
    let collateral_to_seize = (debt_to_cover as u128)
        .checked_mul((10000 + treasury.liquidation_bonus_bps as u64) as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_mul(pool_tokens as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000_u128.checked_mul(pool_sol as u128).ok_or(TorchMarketError::MathOverflow)?)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let actual_collateral_seized = collateral_to_seize.min(loan.collateral_amount);
    let actual_debt_covered = if collateral_to_seize > loan.collateral_amount {
        calculate_collateral_value(actual_collateral_seized, pool_sol, pool_tokens)?
    } else {
        debt_to_cover
    };

    // Simplifies to: debt_to_cover - actual_debt_covered (shortfall on this liquidation call).
    // saturating_sub returns 0 when actual_debt_covered >= debt_to_cover (no bad debt).
    let bad_debt = total_debt.saturating_sub(
        actual_debt_covered.checked_add(
            total_debt.saturating_sub(debt_to_cover)
        ).ok_or(TorchMarketError::MathOverflow)?
    );

    let treasury_seeds = &[
        TREASURY_SEED,
        mint_key.as_ref(),
        &[treasury.bump],
    ];
    let signer_seeds = &[&treasury_seeds[..]];
    if actual_collateral_seized > 0 {
        let collateral_destination = if ctx.accounts.vault_token_account.is_some() {
            ctx.accounts.vault_token_account.as_ref().unwrap().to_account_info()
        } else {
            ctx.accounts.liquidator_token_account.to_account_info()
        };

        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.collateral_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: collateral_destination,
                    authority: ctx.accounts.treasury.to_account_info(),
                },
                signer_seeds,
            ),
            actual_collateral_seized,
            TOKEN_DECIMALS,
        )?;
    }

    if actual_debt_covered > 0 {
        if ctx.accounts.torch_vault.is_some() {
            let vault = ctx.accounts.torch_vault.as_ref().unwrap();
            require!(
                vault.sol_balance >= actual_debt_covered,
                TorchMarketError::InsufficientVaultBalance
            );

            let vault_info = vault.to_account_info();
            let treasury_info = ctx.accounts.treasury.to_account_info();

            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_sub(actual_debt_covered)
                .ok_or(TorchMarketError::MathOverflow)?;
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_add(actual_debt_covered)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_sub(actual_debt_covered)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_spent = vault
                .total_spent
                .checked_add(actual_debt_covered)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            anchor_lang::system_program::transfer(
                CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    anchor_lang::system_program::Transfer {
                        from: ctx.accounts.liquidator.to_account_info(),
                        to: ctx.accounts.treasury.to_account_info(),
                    },
                ),
                actual_debt_covered,
            )?;
        }
    }

    let mut remaining_debt_paid = actual_debt_covered;
    let interest_paid;
    if remaining_debt_paid <= loan.accrued_interest {
        interest_paid = remaining_debt_paid;
        loan.accrued_interest = loan.accrued_interest.saturating_sub(remaining_debt_paid);
        remaining_debt_paid = 0;
    } else {
        interest_paid = loan.accrued_interest;
        remaining_debt_paid -= loan.accrued_interest;
        loan.accrued_interest = 0;
        loan.borrowed_amount = loan.borrowed_amount.saturating_sub(remaining_debt_paid);
    }

    loan.collateral_amount = loan
        .collateral_amount
        .saturating_sub(actual_collateral_seized);
    if bad_debt > 0 {
        loan.borrowed_amount = loan.borrowed_amount.saturating_sub(bad_debt);
        loan.accrued_interest = 0;
    }

    let fully_liquidated = loan.borrowed_amount == 0 && loan.accrued_interest == 0;
    let treasury = &mut ctx.accounts.treasury;
    treasury.total_sol_lent = treasury
        .total_sol_lent
        .saturating_sub(remaining_debt_paid)
        .saturating_sub(bad_debt);
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(actual_debt_covered)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_interest_collected = treasury
        .total_interest_collected
        .checked_add(interest_paid)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.total_collateral_locked = treasury
        .total_collateral_locked
        .saturating_sub(actual_collateral_seized);

    if fully_liquidated {
        treasury.active_loans = treasury.active_loans.saturating_sub(1);
    }

    emit!(LoanLiquidated {
        mint: mint_key,
        borrower: loan.user,
        liquidator: ctx.accounts.liquidator.key(),
        debt_covered: actual_debt_covered,
        collateral_seized: actual_collateral_seized,
        bad_debt,
    });

    Ok(())
}

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
