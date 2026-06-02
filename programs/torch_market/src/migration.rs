use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    burn, set_authority, spl_token_2022::instruction::AuthorityType, transfer_checked, Burn,
    SetAuthority, TransferChecked,
};

use crate::constants::*;
use crate::contexts::MigrateToDex;
use crate::errors::TorchMarketError;
use crate::pool_validation::{read_token_account_balance, treasury_physical_sol};

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

// Migrate bonded token to DeepPool.
// Permissionless — anyone can call once bonding completes.
// The bonded SOL is sourced directly from the System-owned bonding_curve_sol PDA
// (seed-signed) as deep_pool create_pool's sol_source — no separate fund step.
// Flow:
// 1. Handle vote vault (burn or return tokens)
// 2. Burn excess tokens not needed for pool
// 3. Transfer tokens from bonding curve vault to payer
// 4. (SOL sourced from bonding_curve_sol directly in create_pool — no staging)
// 5. CPI to DeepPool create_pool
// 6. Burn LP tokens (lock liquidity forever)
// 7. Revoke mint/freeze/transfer_fee authorities
// 8. Reimburse payer from treasury (direct lamport manipulation — after all CPIs)
// 9. Record baseline from pool state
pub fn migrate_to_dex_handler(ctx: Context<MigrateToDex>) -> Result<()> {
    let bonding_curve = &ctx.accounts.bonding_curve;
    let mint_key = ctx.accounts.mint.key();
    // Migration fee floor — derived from treasury_sol_vault (replaces the context
    // constraint on the removed `sol_balance` field).
    require!(
        treasury_physical_sol(&ctx.accounts.treasury_sol_vault)? >= MIN_MIGRATION_SOL,
        TorchMarketError::InsufficientMigrationFee
    );
    let bc_seeds = &[BONDING_CURVE_SEED, mint_key.as_ref(), &[bonding_curve.bump]];
    let bc_signer = &[&bc_seeds[..]][..];

    // Calculate pool amounts and burn excess tokens
    ctx.accounts.token_vault.reload()?;

    let sol_amount = bonding_curve.real_sol_reserves;
    let vault_token_amount = ctx.accounts.token_vault.amount;
    let tokens_for_pool = crate::math::calc_tokens_for_pool(
        sol_amount,
        bonding_curve.virtual_token_reserves,
        bonding_curve.virtual_sol_reserves,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
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

    // 4. Bonded SOL already lives in bonding_curve_sol (it accumulated there on
    //    every buy) — no staging step needed.

    // 5. CPI to DeepPool create_pool. The pool SOL is sourced from the System-owned
    // bonding_curve_sol (seed-signed), NOT the payer — the bonded raise never transits
    // a user wallet. The payer is still `creator` (rent payer / token source / LP
    // receiver / signer); it only fronts rent, reimbursed below.
    let payer_lamports_pre = ctx.accounts.payer.to_account_info().lamports();
    let second_transfer_fee = calculate_transfer_fee(tokens_payer_will_receive)?;
    let tokens_in_pool = tokens_payer_will_receive
        .checked_sub(second_transfer_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    // Torch config PDA — signer-verified namespace for DeepPool pools.
    let (_, config_bump) = Pubkey::find_program_address(&[TORCH_CONFIG_SEED], &crate::ID);
    let config_seeds = &[TORCH_CONFIG_SEED, &[config_bump]];
    // bonding_curve_sol PDA — address + bump validated here (not in the context, to
    // spare try_accounts stack), then seed-signed as create_pool's sol_source.
    let (bcsol_pda, bcsol_bump) =
        Pubkey::find_program_address(&[BONDING_CURVE_SOL_SEED, mint_key.as_ref()], &crate::ID);
    require!(
        ctx.accounts.bonding_curve_sol.key() == bcsol_pda,
        TorchMarketError::InvalidPoolAccount
    );
    let bcsol_seeds = &[BONDING_CURVE_SOL_SEED, mint_key.as_ref(), &[bcsol_bump]];
    let create_pool_signers = &[&config_seeds[..], &bcsol_seeds[..]];
    let cpi_accounts = deep_pool::cpi::accounts::CreatePool {
        creator: ctx.accounts.payer.to_account_info(),
        sol_source: ctx.accounts.bonding_curve_sol.to_account_info(),
        config: ctx.accounts.torch_config.to_account_info(),
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
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };

    deep_pool::cpi::create_pool(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            cpi_accounts,
            create_pool_signers,
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

    // 8. Reimburse payer's rent from treasury (seed-signed transfer — after all CPIs).
    // The pool SOL came from bonding_curve_sol, not the payer, so the payer's only
    // outlay is the rent it fronted for the new pool accounts (no sol_amount to net out).
    let payer_lamports_post = ctx.accounts.payer.to_account_info().lamports();
    let migration_cost = payer_lamports_pre
        .checked_sub(payer_lamports_post)
        .ok_or(TorchMarketError::MathOverflow)?;

    if migration_cost > 0 {
        // treasury_sol_vault is System-owned → seed-signed system transfer.
        let tsv_seeds: &[&[u8]] = &[
            TREASURY_SOL_VAULT_SEED,
            mint_key.as_ref(),
            &[ctx.bumps.treasury_sol_vault],
        ];
        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.treasury_sol_vault.to_account_info(),
                    to: ctx.accounts.payer.to_account_info(),
                },
                &[tsv_seeds],
            ),
            migration_cost,
        )?;
    }

    // 9. Update state and record baseline (physical SOL already moved; no field)
    let bonding_curve = &mut ctx.accounts.bonding_curve;
    let treasury = &mut ctx.accounts.treasury;
    bonding_curve.migrated = true;
    bonding_curve.real_sol_reserves = 0;
    bonding_curve.real_token_reserves = 0;

    treasury.baseline_sol_reserves = sol_amount;
    treasury.baseline_token_reserves = tokens_in_pool;
    treasury.baseline_initialized = true;
    treasury.min_buyback_interval_slots = DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS;

    emit_cpi!(MigratedToDex {
        mint: ctx.accounts.mint.key(),
        deep_pool: ctx.accounts.deep_pool.key(),
        sol_seeded: sol_amount,
        tokens_seeded: tokens_in_pool,
        lp_burned: lp_amount,
    });

    Ok(())
}

#[event]
pub struct MigratedToDex {
    pub mint: Pubkey,
    pub deep_pool: Pubkey,
    pub sol_seeded: u64,
    pub tokens_seeded: u64,
    pub lp_burned: u64,
}
