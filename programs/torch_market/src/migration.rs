use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, burn, set_authority,
    TransferChecked, Burn, SetAuthority,
    spl_token_2022::instruction::AuthorityType,
};

use crate::constants::*;
use crate::contexts::{FundMigrationSol, MigrateToDex};
use crate::errors::TorchMarketError;
use crate::token_2022_utils::get_associated_token_address_2022;
use crate::pool_validation::read_token_account_balance;

// Calculate transfer fee for our Token-2022 token
// Uses known constants: TRANSFER_FEE_BPS (4 = 0.04%) and MAX_TRANSFER_FEE (u64::MAX)
// Formula: fee = min(ceil(amount * bps / 10000), max_fee)
// Token-2022 uses CEILING division, so we must match that
fn calculate_transfer_fee(amount: u64) -> Result<u64> {
    let numerator = (amount as u128)
        .checked_mul(TRANSFER_FEE_BPS as u128)
        .ok_or(TorchMarketError::MathOverflow)?;
    let fee = numerator
        .checked_add(9999) // Add (10000 - 1) for ceiling
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)? as u64;

    Ok(fee.min(MAX_TRANSFER_FEE))
}

// Fund payer with bonding curve SOL for DeepPool pool creation.
// Separate instruction — isolates direct lamport manipulation from CPIs.
// Called BEFORE migrate_to_dex in the same transaction.
pub fn fund_migration_sol_handler(ctx: Context<FundMigrationSol>) -> Result<()> {
    let sol_amount = ctx.accounts.bonding_curve.real_sol_reserves;
    let bc_info = ctx.accounts.bonding_curve.to_account_info();
    let payer_info = ctx.accounts.payer.to_account_info();

    **bc_info.try_borrow_mut_lamports()? = bc_info
        .lamports()
        .checked_sub(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **payer_info.try_borrow_mut_lamports()? = payer_info
        .lamports()
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Migrate bonded token to DeepPool.
// Permissionless — anyone can call once bonding completes.
// Must be preceded by fund_migration_sol in the same transaction.
// Flow:
// 1. Handle vote vault (burn or return tokens)
// 2. Burn excess tokens not needed for pool
// 3. Transfer tokens from bonding curve vault to payer
// 4. (SOL already in payer via fund_migration_sol)
// 5. CPI to DeepPool create_pool
// 6. Burn LP tokens (lock liquidity forever)
// 7. Revoke mint/freeze/transfer_fee authorities
// 8. Reimburse payer from treasury (direct lamport manipulation — after all CPIs)
// 9. Record baseline from pool state
pub fn migrate_to_dex_handler(ctx: Context<MigrateToDex>) -> Result<()> {
    let bonding_curve = &ctx.accounts.bonding_curve;
    let treasury = &ctx.accounts.treasury;
    let mint_key = ctx.accounts.mint.key();
    let bc_seeds = &[
        BONDING_CURVE_SEED,
        mint_key.as_ref(),
        &[bonding_curve.bump],
    ];
    let bc_signer = &[&bc_seeds[..]][..];

    // 1. Handle vote vault tokens
    let vote_vault_amount = ctx.accounts.treasury_token_account.amount;
    let treasury_seeds = &[
        TREASURY_SEED,
        mint_key.as_ref(),
        &[treasury.bump],
    ];
    let treasury_signer = &[&treasury_seeds[..]][..];
    if vote_vault_amount > 0 {
        if bonding_curve.vote_result_return {
            let expected_lock_ata = get_associated_token_address_2022(
                &ctx.accounts.treasury_lock.key(),
                &mint_key,
            );
            require!(
                ctx.accounts.treasury_lock_token_account.key() == expected_lock_ata,
                TorchMarketError::InvalidTokenAccount
            );

            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_2022_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.treasury_token_account.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                        authority: ctx.accounts.treasury.to_account_info(),
                    },
                    treasury_signer,
                ),
                vote_vault_amount,
                TOKEN_DECIMALS,
            )?;
        } else {
            burn(
                CpiContext::new_with_signer(
                    ctx.accounts.token_2022_program.to_account_info(),
                    Burn {
                        mint: ctx.accounts.mint.to_account_info(),
                        from: ctx.accounts.treasury_token_account.to_account_info(),
                        authority: ctx.accounts.treasury.to_account_info(),
                    },
                    treasury_signer,
                ),
                vote_vault_amount,
            )?;
        }
    }

    // 2. Calculate pool amounts and burn excess tokens
    ctx.accounts.token_vault.reload()?;

    let sol_amount = bonding_curve.real_sol_reserves;
    let vault_token_amount = ctx.accounts.token_vault.amount;
    let tokens_for_pool = (sol_amount as u128)
        .checked_mul(bonding_curve.virtual_token_reserves as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(bonding_curve.virtual_sol_reserves as u128)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let token_amount = tokens_for_pool.min(vault_token_amount);
    let excess_tokens = vault_token_amount
        .checked_sub(token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    if excess_tokens > 0 {
        burn(
            CpiContext::new_with_signer(
                ctx.accounts.token_2022_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.mint.to_account_info(),
                    from: ctx.accounts.token_vault.to_account_info(),
                    authority: ctx.accounts.bonding_curve.to_account_info(),
                },
                bc_signer,
            ),
            excess_tokens,
        )?;
    }

    // 3. Transfer tokens from bonding curve vault to payer's token account
    let transfer_fee = calculate_transfer_fee(token_amount)?;
    let tokens_payer_will_receive = token_amount
        .checked_sub(transfer_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.token_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.payer_token.to_account_info(),
                authority: ctx.accounts.bonding_curve.to_account_info(),
            },
            bc_signer,
        ),
        token_amount,
        TOKEN_DECIMALS,
    )?;

    // 4. SOL already in payer via fund_migration_sol (separate instruction)

    // 5. CPI to DeepPool create_pool
    let payer_lamports_pre = ctx.accounts.payer.to_account_info().lamports();

    let cpi_accounts = deep_pool::cpi::accounts::CreatePool {
        creator: ctx.accounts.payer.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        lp_mint: ctx.accounts.deep_pool_lp_mint.to_account_info(),
        creator_token_account: ctx.accounts.payer_token.to_account_info(),
        creator_lp_account: ctx.accounts.payer_lp_account.to_account_info(),
        pool_lp_account: ctx.accounts.deep_pool_lp_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
    };

    let second_transfer_fee = calculate_transfer_fee(tokens_payer_will_receive)?;
    let tokens_in_pool = tokens_payer_will_receive
        .checked_sub(second_transfer_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    deep_pool::cpi::create_pool(
        CpiContext::new(
            ctx.accounts.deep_pool_program.to_account_info(),
            cpi_accounts,
        ),
        deep_pool::CreatePoolArgs {
            initial_token_amount: tokens_payer_will_receive,
            initial_sol_amount: sol_amount,
        },
    )?;

    // 6. Burn LP tokens — lock liquidity forever
    let lp_amount = read_token_account_balance(&ctx.accounts.payer_lp_account)?;
    if lp_amount > 0 {
        burn(
            CpiContext::new(
                ctx.accounts.token_2022_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.deep_pool_lp_mint.to_account_info(),
                    from: ctx.accounts.payer_lp_account.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            lp_amount,
        )?;
    }

    // 7. Revoke authorities
    set_authority(
        CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            SetAuthority {
                current_authority: ctx.accounts.bonding_curve.to_account_info(),
                account_or_mint: ctx.accounts.mint.to_account_info(),
            },
            bc_signer,
        ),
        AuthorityType::MintTokens,
        None,
    )?;

    {
        let mint_data = ctx.accounts.mint.to_account_info();
        let mint_bytes = mint_data.try_borrow_data()?;
        let has_freeze_authority = mint_bytes.len() > 46 && mint_bytes[46] == 1;
        if has_freeze_authority {
            set_authority(
                CpiContext::new_with_signer(
                    ctx.accounts.token_2022_program.to_account_info(),
                    SetAuthority {
                        current_authority: ctx.accounts.bonding_curve.to_account_info(),
                        account_or_mint: ctx.accounts.mint.to_account_info(),
                    },
                    bc_signer,
                ),
                AuthorityType::FreezeAccount,
                None,
            )?;
        }
    }

    set_authority(
        CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            SetAuthority {
                current_authority: ctx.accounts.bonding_curve.to_account_info(),
                account_or_mint: ctx.accounts.mint.to_account_info(),
            },
            bc_signer,
        ),
        AuthorityType::TransferFeeConfig,
        None,
    )?;

    // 8. Reimburse payer from treasury (direct lamport manipulation — after all CPIs)
    let payer_lamports_post = ctx.accounts.payer.to_account_info().lamports();
    // Subtract sol_amount: that SOL came from the bonding curve (via fund_migration_sol),
    // not from the payer's wallet. Only reimburse the rent for new accounts.
    let migration_cost = payer_lamports_pre
        .checked_sub(payer_lamports_post)
        .ok_or(TorchMarketError::MathOverflow)?
        .saturating_sub(sol_amount);

    {
        let treasury_info = ctx.accounts.treasury.to_account_info();
        let payer_info = ctx.accounts.payer.to_account_info();
        **treasury_info.try_borrow_mut_lamports()? = treasury_info
            .lamports()
            .checked_sub(migration_cost)
            .ok_or(TorchMarketError::InsufficientMigrationFee)?;
        **payer_info.try_borrow_mut_lamports()? = payer_info
            .lamports()
            .checked_add(migration_cost)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    // 9. Update state and record baseline
    let bonding_curve = &mut ctx.accounts.bonding_curve;
    let treasury = &mut ctx.accounts.treasury;
    bonding_curve.migrated = true;
    bonding_curve.real_sol_reserves = 0;
    bonding_curve.real_token_reserves = 0;
    bonding_curve.vote_vault_balance = 0;
    treasury.sol_balance = treasury.sol_balance
        .checked_sub(migration_cost)
        .ok_or(TorchMarketError::InsufficientMigrationFee)?;

    treasury.baseline_sol_reserves = sol_amount;
    treasury.baseline_token_reserves = tokens_in_pool;
    treasury.baseline_initialized = true;
    treasury.min_buyback_interval_slots = DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS;

    Ok(())
}
