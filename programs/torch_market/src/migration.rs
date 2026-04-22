use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke;
use anchor_spl::token::spl_token;
use anchor_spl::token_interface::{
    burn, close_account, set_authority, spl_token_2022::instruction::AuthorityType,
    transfer_checked, Burn, CloseAccount, SetAuthority, TransferChecked,
};

use crate::constants::*;
use crate::contexts::{FundMigrationWsol, MigrateToDex};
use crate::errors::TorchMarketError;
use crate::token_2022_utils::get_associated_token_address_2022;

pub use raydium_cpmm_cpi;

// Native SOL mint (WSOL): So11111111111111111111111111111111111111112
pub const WSOL_MINT: Pubkey = Pubkey::new_from_array([
    6, 155, 136, 87, 254, 171, 129, 132, 251, 104, 127, 99, 70, 24, 192, 53, 218, 196, 57, 220, 26,
    235, 59, 85, 152, 160, 240, 0, 0, 0, 0, 1,
]);

// Order tokens for Raydium (token_0 < token_1 by pubkey)
// Returns (token_0, token_1, is_wsol_token_0)
pub fn order_tokens_for_raydium(wsol_mint: &Pubkey, token_mint: &Pubkey) -> (Pubkey, Pubkey, bool) {
    if wsol_mint < token_mint {
        (*wsol_mint, *token_mint, true) // WSOL is token_0
    } else {
        (*token_mint, *wsol_mint, false) // Token is token_0
    }
}

// Calculate transfer fee for our Token-2022 token
// Uses known constants: TRANSFER_FEE_BPS (100 = 1%) and MAX_TRANSFER_FEE (u64::MAX)
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

// Fund bonding curve's WSOL ATA with bonding curve SOL.
// Separate instruction — isolates direct lamport manipulation from CPIs.
// SOL stays in protocol-controlled bc_wsol until migrate_to_dex closes it.
// Called BEFORE migrate_to_dex in the same transaction.
pub fn fund_migration_wsol_handler(ctx: Context<FundMigrationWsol>) -> Result<()> {
    let sol_amount = ctx.accounts.bonding_curve.real_sol_reserves;
    let bc_info = ctx.accounts.bonding_curve.to_account_info();
    let wsol_info = ctx.accounts.bc_wsol.to_account_info();

    **bc_info.try_borrow_mut_lamports()? = bc_info
        .lamports()
        .checked_sub(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **wsol_info.try_borrow_mut_lamports()? = wsol_info
        .lamports()
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// Migrate bonded token to Raydium CPMM
// Permissionless — bc_wsol must be pre-funded via fund_migration_wsol.
// No direct lamport manipulation — all SOL movement via CPIs only.
pub fn migrate_to_dex_handler(ctx: Context<MigrateToDex>) -> Result<()> {
    let bonding_curve = &ctx.accounts.bonding_curve;
    let treasury = &ctx.accounts.treasury;
    let mint_key = ctx.accounts.mint.key();
    let bc_seeds = &[BONDING_CURVE_SEED, mint_key.as_ref(), &[bonding_curve.bump]];
    let bc_signer = &[&bc_seeds[..]][..];

    close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        CloseAccount {
            account: ctx.accounts.bc_wsol.to_account_info(),
            destination: ctx.accounts.payer_wsol.to_account_info(),
            authority: ctx.accounts.bonding_curve.to_account_info(),
        },
        bc_signer,
    ))?;

    invoke(
        &spl_token::instruction::sync_native(
            &ctx.accounts.token_program.key(),
            &ctx.accounts.payer_wsol.key(),
        )?,
        &[ctx.accounts.payer_wsol.to_account_info()],
    )?;

    let vote_vault_amount = ctx.accounts.treasury_token_account.amount;
    let treasury_seeds = &[TREASURY_SEED, mint_key.as_ref(), &[treasury.bump]];
    let treasury_signer = &[&treasury_seeds[..]][..];
    if vote_vault_amount > 0 {
        if bonding_curve.vote_result_return {
            // [V31] Validate treasury_lock_token_account is the correct ATA
            let expected_lock_ata =
                get_associated_token_address_2022(&ctx.accounts.treasury_lock.key(), &mint_key);
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

    let transfer_fee = calculate_transfer_fee(token_amount)?;
    let tokens_payer_will_receive = token_amount
        .checked_sub(transfer_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    let (_token_0, _token_1, is_wsol_token_0) =
        order_tokens_for_raydium(&WSOL_MINT, &ctx.accounts.mint.key());
    let (init_amount_0, init_amount_1) = if is_wsol_token_0 {
        (sol_amount, tokens_payer_will_receive) // WSOL is token_0, Token is token_1
    } else {
        (tokens_payer_will_receive, sol_amount) // Token is token_0, WSOL is token_1
    };

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

    let payer_lamports_pre = ctx.accounts.payer.to_account_info().lamports();
    let cpi_accounts = if is_wsol_token_0 {
        raydium_cpmm_cpi::cpi::accounts::Initialize {
            creator: ctx.accounts.payer.to_account_info(),
            amm_config: ctx.accounts.amm_config.to_account_info(),
            authority: ctx.accounts.raydium_authority.to_account_info(),
            pool_state: ctx.accounts.pool_state.to_account_info(),
            token_0_mint: ctx.accounts.wsol_mint.to_account_info(),
            token_1_mint: ctx.accounts.mint.to_account_info(),
            lp_mint: ctx.accounts.lp_mint.to_account_info(),
            creator_token_0: ctx.accounts.payer_wsol.to_account_info(),
            creator_token_1: ctx.accounts.payer_token.to_account_info(),
            creator_lp_token: ctx.accounts.payer_lp_token.to_account_info(),
            token_0_vault: ctx.accounts.token_0_vault.to_account_info(),
            token_1_vault: ctx.accounts.token_1_vault.to_account_info(),
            create_pool_fee: ctx.accounts.create_pool_fee.to_account_info(),
            observation_state: ctx.accounts.observation_state.to_account_info(),
            token_program: ctx.accounts.token_program.to_account_info(),
            token_0_program: ctx.accounts.token_program.to_account_info(), // WSOL = SPL Token
            token_1_program: ctx.accounts.token_2022_program.to_account_info(), // Token = Token-2022
            associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            rent: ctx.accounts.rent.to_account_info(),
        }
    } else {
        raydium_cpmm_cpi::cpi::accounts::Initialize {
            creator: ctx.accounts.payer.to_account_info(),
            amm_config: ctx.accounts.amm_config.to_account_info(),
            authority: ctx.accounts.raydium_authority.to_account_info(),
            pool_state: ctx.accounts.pool_state.to_account_info(),
            token_0_mint: ctx.accounts.mint.to_account_info(),
            token_1_mint: ctx.accounts.wsol_mint.to_account_info(),
            lp_mint: ctx.accounts.lp_mint.to_account_info(),
            creator_token_0: ctx.accounts.payer_token.to_account_info(),
            creator_token_1: ctx.accounts.payer_wsol.to_account_info(),
            creator_lp_token: ctx.accounts.payer_lp_token.to_account_info(),
            token_0_vault: ctx.accounts.token_0_vault.to_account_info(),
            token_1_vault: ctx.accounts.token_1_vault.to_account_info(),
            create_pool_fee: ctx.accounts.create_pool_fee.to_account_info(),
            observation_state: ctx.accounts.observation_state.to_account_info(),
            token_program: ctx.accounts.token_program.to_account_info(),
            token_0_program: ctx.accounts.token_2022_program.to_account_info(), // Token = Token-2022
            token_1_program: ctx.accounts.token_program.to_account_info(),      // WSOL = SPL Token
            associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            rent: ctx.accounts.rent.to_account_info(),
        }
    };

    let cpi_ctx = CpiContext::new(ctx.accounts.raydium_program.to_account_info(), cpi_accounts);

    raydium_cpmm_cpi::cpi::initialize(cpi_ctx, init_amount_0, init_amount_1, 0)?;

    let lp_amount = {
        let data = ctx.accounts.payer_lp_token.try_borrow_data()?;
        if data.len() >= 72 {
            u64::from_le_bytes(data[64..72].try_into().unwrap())
        } else {
            0
        }
    };

    if lp_amount > 0 {
        burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    from: ctx.accounts.payer_lp_token.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            lp_amount,
        )?;
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

    let payer_lamports_post = ctx.accounts.payer.to_account_info().lamports();
    let migration_cost = payer_lamports_pre
        .checked_sub(payer_lamports_post)
        .ok_or(TorchMarketError::MathOverflow)?;

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

    let bonding_curve = &mut ctx.accounts.bonding_curve;
    let treasury = &mut ctx.accounts.treasury;
    bonding_curve.migrated = true;
    bonding_curve.real_sol_reserves = 0;
    bonding_curve.real_token_reserves = 0;
    bonding_curve.vote_vault_balance = 0;
    treasury.sol_balance = treasury
        .sol_balance
        .checked_sub(migration_cost)
        .ok_or(TorchMarketError::InsufficientMigrationFee)?;

    let second_transfer_fee = calculate_transfer_fee(tokens_payer_will_receive)?;
    let tokens_in_pool = tokens_payer_will_receive
        .checked_sub(second_transfer_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.baseline_sol_reserves = sol_amount;
    treasury.baseline_token_reserves = tokens_in_pool;
    treasury.baseline_initialized = true;
    treasury.min_buyback_interval_slots = DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS;

    Ok(())
}
