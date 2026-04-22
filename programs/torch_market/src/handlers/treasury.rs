use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::read_deep_pool_reserves;
use crate::token_2022_utils::*;

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

// Swap treasury tokens to SOL via DeepPool.
// Ratio-gated: only sells when price is 20%+ above baseline.
// Sells 15% of held tokens per call (or 100% if balance <= 1M tokens).
// Shares cooldown with buyback — prevents rapid buy/sell cycles.
// Pre-baseline tokens (migrated before V9) bypass ratio gating.
// Flow:
// 1. Read DeepPool reserves (pool lamports + vault balance)
// 2. Ratio gate: check price is 20%+ above baseline
// 3. Calculate sell amount (15% or 100% if small balance)
// 4. Record treasury lamports before swap
// 5. DeepPool swap CPI (Token-2022 → SOL)
// 6. Measure SOL received from lamport delta
// 7. Creator fee split (direct lamport manipulation — after CPI)
// 8. Update treasury state + shared cooldown
pub fn swap_fees_to_sol(ctx: Context<SwapFeesToSol>, minimum_amount_out: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let current_slot = Clock::get()?.slot;

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

        let (pool_sol, pool_tokens) = read_deep_pool_reserves(
            &ctx.accounts.deep_pool,
            &ctx.accounts.deep_pool_token_vault,
        )?;
        require!(pool_tokens > 0, TorchMarketError::ZeroPoolReserves);

        let current_ratio = (pool_sol as u128)
            .checked_mul(RATIO_PRECISION)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(pool_tokens as u128)
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

    // Record treasury lamports before swap
    let treasury_lamports_before = ctx.accounts.treasury.to_account_info().lamports();

    // DeepPool swap CPI: sell tokens for SOL
    let treasury_seeds = &[
        TREASURY_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury.bump],
    ];
    let treasury_signer = &[&treasury_seeds[..]][..];

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.treasury.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.treasury_token_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
    };

    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            treasury_signer,
        ),
        deep_pool::SwapArgs {
            amount_in: sell_amount,
            minimum_out: minimum_amount_out,
            buy: false,
        },
    )?;

    // Measure SOL received from lamport delta
    let treasury_lamports_after = ctx.accounts.treasury.to_account_info().lamports();
    let sol_received = treasury_lamports_after
        .checked_sub(treasury_lamports_before)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        sol_received >= minimum_amount_out,
        TorchMarketError::SlippageExceeded
    );

    // Creator fee split (direct lamport manipulation — after all CPIs)
    let is_community_token = ctx.accounts.treasury.is_community_token;
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
    treasury.last_buyback_slot = current_slot;

    Ok(())
}
