use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::{
    order_mints, read_pool_accumulated_fees, read_token_account_balance, validate_pool_accounts,
};
use crate::token_2022_utils::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::spl_token;

// Harvest accumulated transfer fees.
// This collects transfer fees that have been withheld from transfers
// into the token treasury. Anyone can call this (permissionless).
// The harvested tokens can be used in future buybacks to reduce supply.
pub fn harvest_fees<'info>(ctx: Context<'_, '_, 'info, 'info, HarvestFees<'info>>) -> Result<()> {
    let bonding_curve = &ctx.accounts.bonding_curve;
    let token_treasury = &mut ctx.accounts.token_treasury;
    let mint_key = ctx.accounts.mint.key();
    require!(bonding_curve.is_token_2022, TorchMarketError::NotToken2022);

    if !ctx.remaining_accounts.is_empty() {
        let source_pubkeys: Vec<Pubkey> = ctx.remaining_accounts.iter().map(|a| a.key()).collect();
        let harvest_ix =
            build_harvest_withheld_tokens_to_mint_instruction(&mint_key, &source_pubkeys);
        let mut harvest_accounts = vec![ctx.accounts.mint.to_account_info()];
        for acc in ctx.remaining_accounts.iter() {
            harvest_accounts.push(acc.to_account_info());
        }

        anchor_lang::solana_program::program::invoke(&harvest_ix, &harvest_accounts)?;
    }

    let treasury_seeds = &[TREASURY_SEED, mint_key.as_ref(), &[token_treasury.bump]];
    let signer_seeds = &[&treasury_seeds[..]];
    let withdraw_ix = build_withdraw_withheld_tokens_from_mint_instruction(
        &mint_key,
        &ctx.accounts.treasury_token_account.key(),
        &token_treasury.key(),
    );

    invoke_signed(
        &withdraw_ix,
        &[
            ctx.accounts.mint.to_account_info(),
            ctx.accounts.treasury_token_account.to_account_info(),
            token_treasury.to_account_info(),
        ],
        signer_seeds,
    )?;

    Ok(())
}

// Swap treasury tokens to SOL via Raydium CPMM.
// Ratio-gated: only sells when price is 20%+ above baseline.
// Sells 15% of held tokens per call (or 100% if balance <= 1M tokens).
// Shares cooldown with buyback — prevents rapid buy/sell cycles.
// Pre-baseline tokens (migrated before V9) bypass ratio gating.
// Flow:
// 1. Validate pool accounts (defense in depth)
// 2. Ratio gate: check price is 20%+ above baseline
// 3. Calculate sell amount (15% or 100% if small balance)
// 4. Read WSOL balance before swap (handles pre-existing WSOL)
// 5. Raydium swap_base_input CPI (Token-2022 → WSOL)
// 6. Slippage check: sol_received >= minimum_amount_out
// 7. Close WSOL ATA → treasury PDA (unwrap to SOL)
// 8. Update treasury state + shared cooldown
pub fn swap_fees_to_sol(ctx: Context<SwapFeesToSol>, minimum_amount_out: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let current_slot = Clock::get()?.slot;
    let (mint_0, _) = order_mints(&mint_key);
    let (vault_0, vault_1) = if mint_0 == mint_key {
        // Our token is token_0, WSOL is token_1
        (&ctx.accounts.token_vault, &ctx.accounts.wsol_vault)
    } else {
        // WSOL is token_0, our token is token_1
        (&ctx.accounts.wsol_vault, &ctx.accounts.token_vault)
    };

    validate_pool_accounts(&ctx.accounts.pool_state, vault_0, vault_1, &mint_key)?;

    let token_amount = ctx.accounts.treasury_token_account.amount;
    require!(token_amount > 0, TorchMarketError::AmountTooSmall);
    require!(minimum_amount_out > 0, TorchMarketError::AmountTooSmall);

    let sell_amount = if ctx.accounts.treasury.baseline_initialized {
        let next_slot = ctx
            .accounts
            .treasury
            .last_buyback_slot
            .checked_add(ctx.accounts.treasury.min_buyback_interval_slots)
            .ok_or(TorchMarketError::MathOverflow)?;
        if ctx.accounts.treasury.last_buyback_slot > 0 && current_slot < next_slot {
            return Ok(());
        }

        let pool_sol_balance = read_token_account_balance(&ctx.accounts.wsol_vault)?;
        let pool_token_balance = read_token_account_balance(&ctx.accounts.token_vault)?;
        let is_wsol_token_0 = mint_0 != mint_key; // if our mint is token_0, WSOL isn't
        let (sol_fees, token_fees) =
            read_pool_accumulated_fees(&ctx.accounts.pool_state, is_wsol_token_0)?;
        let pool_sol_balance = pool_sol_balance.saturating_sub(sol_fees);
        let pool_token_balance = pool_token_balance.saturating_sub(token_fees);
        require!(pool_token_balance > 0, TorchMarketError::ZeroPoolReserves);

        let current_ratio = (pool_sol_balance as u128)
            .checked_mul(RATIO_PRECISION)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(pool_token_balance as u128)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        let baseline_ratio = (ctx.accounts.treasury.baseline_sol_reserves as u128)
            .checked_mul(RATIO_PRECISION)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(ctx.accounts.treasury.baseline_token_reserves as u128)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        let sell_threshold = (baseline_ratio as u128)
            .checked_mul(DEFAULT_SELL_THRESHOLD_BPS as u128)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(10000)
            .ok_or(TorchMarketError::MathOverflow)? as u64;
        if current_ratio < sell_threshold {
            return Ok(());
        }

        if token_amount <= SELL_ALL_TOKEN_THRESHOLD {
            token_amount
        } else {
            (token_amount as u128)
                .checked_mul(DEFAULT_SELL_PERCENT_BPS as u128)
                .ok_or(TorchMarketError::MathOverflow)?
                .checked_div(10000)
                .ok_or(TorchMarketError::MathOverflow)? as u64
        }
    } else {
        token_amount
    };

    if sell_amount == 0 {
        return Ok(());
    }

    let wsol_balance_before = read_token_account_balance(&ctx.accounts.treasury_wsol)?;
    let treasury_seeds = &[
        TREASURY_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury.bump],
    ];
    let treasury_signer = &[&treasury_seeds[..]][..];
    let swap_accounts = raydium_cpmm_cpi::cpi::accounts::Swap {
        payer: ctx.accounts.treasury.to_account_info(),
        authority: ctx.accounts.raydium_authority.to_account_info(),
        amm_config: ctx.accounts.amm_config.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        input_token_account: ctx.accounts.treasury_token_account.to_account_info(),
        output_token_account: ctx.accounts.treasury_wsol.to_account_info(),
        input_vault: ctx.accounts.token_vault.to_account_info(),
        output_vault: ctx.accounts.wsol_vault.to_account_info(),
        input_token_program: ctx.accounts.token_2022_program.to_account_info(),
        output_token_program: ctx.accounts.token_program.to_account_info(),
        input_token_mint: ctx.accounts.mint.to_account_info(),
        output_token_mint: ctx.accounts.wsol_mint.to_account_info(),
        observation_state: ctx.accounts.observation_state.to_account_info(),
    };

    raydium_cpmm_cpi::cpi::swap_base_input(
        CpiContext::new_with_signer(
            ctx.accounts.raydium_program.to_account_info(),
            swap_accounts,
            treasury_signer,
        ),
        sell_amount,
        minimum_amount_out,
    )?;

    let wsol_balance_after = read_token_account_balance(&ctx.accounts.treasury_wsol)?;
    let sol_received = wsol_balance_after
        .checked_sub(wsol_balance_before)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        sol_received >= minimum_amount_out,
        TorchMarketError::SlippageExceeded
    );

    invoke_signed(
        &spl_token::instruction::close_account(
            &ctx.accounts.token_program.key(),
            &ctx.accounts.treasury_wsol.key(),
            &ctx.accounts.treasury.key(),
            &ctx.accounts.treasury.key(),
            &[],
        )?,
        &[
            ctx.accounts.treasury_wsol.to_account_info(),
            ctx.accounts.treasury.to_account_info(),
            ctx.accounts.treasury.to_account_info(),
        ],
        treasury_signer,
    )?;

    let is_community_token = ctx.accounts.treasury.total_bought_back == COMMUNITY_TOKEN_SENTINEL;
    let (creator_amount, treasury_amount) = if is_community_token {
        (0u64, sol_received)
    } else {
        let ca = crate::math::calc_creator_fee_share(sol_received)
            .ok_or(TorchMarketError::MathOverflow)?;
        let ta = sol_received
            .checked_sub(ca)
            .ok_or(TorchMarketError::MathOverflow)?;
        (ca, ta)
    };

    if creator_amount > 0 {
        let treasury_info = ctx.accounts.treasury.to_account_info();
        **treasury_info.try_borrow_mut_lamports()? = treasury_info
            .lamports()
            .checked_sub(creator_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        let creator_info = ctx.accounts.creator.to_account_info();
        **creator_info.try_borrow_mut_lamports()? = creator_info
            .lamports()
            .checked_add(creator_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let treasury = &mut ctx.accounts.treasury;
    treasury.sol_balance = treasury
        .sol_balance
        .checked_add(treasury_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.harvested_fees = treasury
        .harvested_fees
        .checked_add(treasury_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.tokens_held = treasury.tokens_held.saturating_sub(sell_amount);
    treasury.last_buyback_slot = current_slot;

    Ok(())
}
