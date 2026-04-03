use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::{invoke, invoke_signed};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::token_2022_utils::*;

// Create a new Token-2022 token with transfer fee extension.
// This creates a Token-2022 mint with a 1% transfer fee that applies to
// ALL transfers for the lifetime of the token. The fee is collected in
// recipient accounts and can be harvested via the harvest_fees instruction.
// Transfer fee configuration:
// - Fee: 1% (100 basis points)
// - Max fee: Unlimited (u64::MAX)
// - Fee config authority: Global config authority
// - Withdraw authority: Token treasury PDA
pub fn create_token(ctx: Context<CreateToken2022>, args: CreateTokenArgs) -> Result<()> {
    require!(args.name.len() <= 32, TorchMarketError::NameTooLong);
    require!(args.symbol.len() <= 10, TorchMarketError::SymbolTooLong);
    require!(args.uri.len() <= 200, TorchMarketError::UriTooLong);

    let bonding_target = if args.sol_target == 0 {
        BONDING_TARGET_LAMPORTS
    } else {
        require!(
            VALID_BONDING_TARGETS.contains(&args.sol_target),
            TorchMarketError::InvalidBondingTarget
        );
        args.sol_target
    };

    let mint_key = ctx.accounts.mint.key();
    let bonding_curve_key = ctx.accounts.bonding_curve.key();
    let treasury_key = ctx.accounts.treasury.key();
    let initial_space = get_mint_with_pointer_space();
    let initial_rent = Rent::get()?.minimum_balance(initial_space);

    invoke(
        &anchor_lang::solana_program::system_instruction::create_account(
            &ctx.accounts.creator.key(),
            &mint_key,
            initial_rent,
            initial_space as u64,
            &TOKEN_2022_PROGRAM_ID,
        ),
        &[
            ctx.accounts.creator.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
    )?;

    let init_fee_ix = build_initialize_transfer_fee_config_instruction(
        &mint_key,
        Some(&bonding_curve_key),
        Some(&treasury_key),
        TRANSFER_FEE_BPS,
        MAX_TRANSFER_FEE,
    );

    invoke(
        &init_fee_ix,
        &[ctx.accounts.mint.to_account_info()],
    )?;

    let init_metadata_pointer_ix = build_initialize_metadata_pointer_instruction(
        &mint_key,
        None,       // no authority — pointer is permanently immutable
        &mint_key,  // metadata lives on mint itself
    );

    invoke(
        &init_metadata_pointer_ix,
        &[ctx.accounts.mint.to_account_info()],
    )?;

    let init_mint_ix = build_initialize_mint2_instruction(
        &mint_key,
        &bonding_curve_key,
        None,
        TOKEN_DECIMALS,
    );

    invoke(
        &init_mint_ix,
        &[ctx.accounts.mint.to_account_info()],
    )?;

    let final_space = get_mint_with_metadata_space(args.name.len(), args.symbol.len(), args.uri.len());
    let final_rent = Rent::get()?.minimum_balance(final_space);
    let additional_rent = final_rent.saturating_sub(initial_rent);
    if additional_rent > 0 {
        invoke(
            &anchor_lang::solana_program::system_instruction::transfer(
                &ctx.accounts.creator.key(),
                &mint_key,
                additional_rent,
            ),
            &[
                ctx.accounts.creator.to_account_info(),
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;
    }

    let seeds = &[
        BONDING_CURVE_SEED,
        mint_key.as_ref(),
        &[ctx.bumps.bonding_curve],
    ];
    let signer_seeds = &[&seeds[..]];
    let init_metadata_ix = build_initialize_token_metadata_instruction(
        &mint_key,
        &bonding_curve_key,    // update authority
        &bonding_curve_key,    // mint authority (signer via PDA)
        &args.name,
        &args.symbol,
        &args.uri,
    );

    invoke_signed(
        &init_metadata_ix,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.bonding_curve.to_account_info(),
        ],
        signer_seeds,
    )?;

    let bonding_curve = &mut ctx.accounts.bonding_curve;
    bonding_curve.mint = mint_key;
    bonding_curve.creator = ctx.accounts.creator.key();

    let (ivs, ivt) = initial_virtual_reserves(bonding_target);
    bonding_curve.virtual_sol_reserves = ivs;
    bonding_curve.virtual_token_reserves = ivt;
    bonding_curve.real_sol_reserves = 0;
    bonding_curve.real_token_reserves = CURVE_SUPPLY;
    bonding_curve.vote_vault_balance = 0; // [V36] Vote vault removed — never incremented
    bonding_curve.permanently_burned_tokens = 0;
    bonding_curve.bonding_complete = false;
    bonding_curve.bonding_complete_slot = 0;
    bonding_curve.votes_return = 0;   // [V36] Unused — kept for layout compat
    bonding_curve.votes_burn = 0;     // [V36] Unused
    bonding_curve.total_voters = 0;   // [V36] Unused
    bonding_curve.vote_finalized = true;  // [V36] Set true so migration gate passes without votes
    bonding_curve.vote_result_return = false; // [V36] Irrelevant when vault is empty
    bonding_curve.migrated = false;
    bonding_curve.is_token_2022 = true; // V3 flag
    bonding_curve.last_activity_slot = Clock::get()?.slot;
    bonding_curve.reclaimed = false;

    let name_bytes = args.name.as_bytes();
    bonding_curve.name[..name_bytes.len()].copy_from_slice(name_bytes);

    let symbol_bytes = args.symbol.as_bytes();
    bonding_curve.symbol[..symbol_bytes.len()].copy_from_slice(symbol_bytes);

    let uri_bytes = args.uri.as_bytes();
    bonding_curve.uri[..uri_bytes.len()].copy_from_slice(uri_bytes);
    bonding_curve.bump = ctx.bumps.bonding_curve;
    bonding_curve.treasury_bump = ctx.bumps.treasury;
    bonding_curve.bonding_target = bonding_target; // [V23]

    let treasury = &mut ctx.accounts.treasury;
    treasury.bonding_curve = bonding_curve_key;
    treasury.mint = mint_key;
    treasury.sol_balance = 0;
    treasury.total_bought_back = if args.community_token {
        COMMUNITY_TOKEN_SENTINEL
    } else {
        0
    };

    treasury.total_burned_from_buyback = 0;
    treasury.tokens_held = 0;
    treasury.last_buyback_slot = 0;
    treasury.buyback_count = 0;
    treasury.harvested_fees = 0; // V3 field
    treasury.bump = ctx.bumps.treasury;
    treasury.baseline_sol_reserves = 0;
    treasury.baseline_token_reserves = 0;
    treasury.ratio_threshold_bps = 0;
    treasury.reserve_ratio_bps = 0;
    treasury.buyback_percent_bps = 0;
    treasury.min_buyback_interval_slots = DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS;
    treasury.baseline_initialized = false;
    treasury.total_stars = 0;
    treasury.star_sol_balance = 0;
    treasury.creator_paid_out = false;

    let treasury_lock = &mut ctx.accounts.treasury_lock;
    treasury_lock.mint = mint_key;
    treasury_lock.bump = ctx.bumps.treasury_lock;
    treasury.total_sol_lent = 0;
    treasury.total_collateral_locked = 0;
    treasury.active_loans = 0;
    treasury.total_interest_collected = 0;
    treasury.lending_enabled = true;
    treasury.interest_rate_bps = DEFAULT_INTEREST_RATE_BPS;
    treasury.max_ltv_bps = DEFAULT_MAX_LTV_BPS;
    treasury.liquidation_threshold_bps = DEFAULT_LIQUIDATION_THRESHOLD_BPS;
    treasury.liquidation_bonus_bps = DEFAULT_LIQUIDATION_BONUS_BPS;
    treasury.liquidation_close_bps = DEFAULT_LIQUIDATION_CLOSE_BPS;
    treasury.lending_utilization_cap_bps = DEFAULT_LENDING_UTILIZATION_CAP_BPS;
    treasury.buyback_percent_bps = SHORT_ENABLED_SENTINEL;
    treasury.total_burned_from_buyback = 0;

    let create_vault_ata_ix = build_create_associated_token_account_instruction(
        &ctx.accounts.creator.key(),
        &bonding_curve_key,
        &mint_key,
    );

    invoke(
        &create_vault_ata_ix,
        &[
            ctx.accounts.creator.to_account_info(),
            ctx.accounts.token_vault.to_account_info(),
            bonding_curve.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.token_2022_program.to_account_info(),
        ],
    )?;

    let create_treasury_ata_ix = build_create_associated_token_account_instruction(
        &ctx.accounts.creator.key(),
        &treasury_key,
        &mint_key,
    );

    invoke(
        &create_treasury_ata_ix,
        &[
            ctx.accounts.creator.to_account_info(),
            ctx.accounts.treasury_token_account.to_account_info(),
            ctx.accounts.treasury.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.token_2022_program.to_account_info(),
        ],
    )?;

    let mint_to_ix = build_mint_to_instruction(
        &mint_key,
        &ctx.accounts.token_vault.key(),
        &bonding_curve_key,
        CURVE_SUPPLY,
    );

    invoke_signed(
        &mint_to_ix,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.token_vault.to_account_info(),
            bonding_curve.to_account_info(),
        ],
        signer_seeds,
    )?;

    let treasury_lock_key = ctx.accounts.treasury_lock.key();
    let create_lock_ata_ix = build_create_associated_token_account_instruction(
        &ctx.accounts.creator.key(),
        &treasury_lock_key,
        &mint_key,
    );

    invoke(
        &create_lock_ata_ix,
        &[
            ctx.accounts.creator.to_account_info(),
            ctx.accounts.treasury_lock_token_account.to_account_info(),
            ctx.accounts.treasury_lock.to_account_info(),
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.token_2022_program.to_account_info(),
        ],
    )?;

    let mint_lock_ix = build_mint_to_instruction(
        &mint_key,
        &ctx.accounts.treasury_lock_token_account.key(),
        &bonding_curve_key,
        TREASURY_LOCK_TOKENS,
    );

    invoke_signed(
        &mint_lock_ix,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.treasury_lock_token_account.to_account_info(),
            bonding_curve.to_account_info(),
        ],
        signer_seeds,
    )?;

    Ok(())
}
