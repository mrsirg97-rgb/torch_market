use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::state::{ProtocolTreasury, UserStats};

pub fn initialize_protocol_treasury(ctx: Context<InitializeProtocolTreasury>) -> Result<()> {
    let protocol_treasury = &mut ctx.accounts.protocol_treasury;
    protocol_treasury.authority = ctx.accounts.authority.key();
    protocol_treasury.current_balance = 0;
    protocol_treasury.reserve_floor = PROTOCOL_TREASURY_RESERVE_FLOOR;
    protocol_treasury.total_fees_received = 0;
    protocol_treasury.total_distributed = 0;
    protocol_treasury.current_epoch = 0;
    protocol_treasury.last_epoch_ts = Clock::get()?.unix_timestamp;
    protocol_treasury.total_volume_current_epoch = 0;
    protocol_treasury.total_volume_previous_epoch = 0;
    protocol_treasury.distributable_amount = 0;
    protocol_treasury.bump = ctx.bumps.protocol_treasury;
    Ok(())
}

pub fn advance_protocol_epoch(ctx: Context<AdvanceProtocolEpoch>) -> Result<()> {
    let current_ts = Clock::get()?.unix_timestamp;
    let time_since_last = current_ts
        .checked_sub(ctx.accounts.protocol_treasury.last_epoch_ts)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        time_since_last >= EPOCH_DURATION_SECONDS,
        TorchMarketError::EpochNotComplete
    );

    ctx.accounts.protocol_treasury.total_volume_previous_epoch =
        ctx.accounts.protocol_treasury.total_volume_current_epoch;

    let actual_balance = ctx.accounts.protocol_treasury.to_account_info().lamports();
    let rent = Rent::get()?;
    let rent_exempt_min = rent.minimum_balance(ProtocolTreasury::LEN);
    let available_balance = actual_balance.saturating_sub(rent_exempt_min);
    let reserve_floor = ctx.accounts.protocol_treasury.reserve_floor;

    ctx.accounts.protocol_treasury.current_balance = available_balance;
    ctx.accounts.protocol_treasury.distributable_amount =
        available_balance.saturating_sub(reserve_floor);
    ctx.accounts.protocol_treasury.total_volume_current_epoch = 0;
    ctx.accounts.protocol_treasury.current_epoch = ctx
        .accounts
        .protocol_treasury
        .current_epoch
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;
    ctx.accounts.protocol_treasury.last_epoch_ts = current_ts;
    Ok(())
}

// Shared: compute claim amount and validate eligibility.
fn compute_claim(
    user_stats: &mut UserStats,
    protocol_treasury: &ProtocolTreasury,
) -> Result<u64> {
    let current_epoch = protocol_treasury.current_epoch;
    require!(current_epoch > 0, TorchMarketError::NoRewardsAvailable);

    if user_stats.last_volume_epoch < current_epoch && user_stats.volume_current_epoch > 0 {
        user_stats.volume_previous_epoch = user_stats.volume_current_epoch;
        user_stats.volume_current_epoch = 0;
        user_stats.last_volume_epoch = current_epoch;
    }

    let claimable_epoch = current_epoch.saturating_sub(1);
    require!(
        user_stats.last_epoch_claimed < claimable_epoch,
        TorchMarketError::AlreadyClaimed
    );
    require!(
        protocol_treasury.distributable_amount > 0,
        TorchMarketError::NoRewardsAvailable
    );
    require!(
        user_stats.volume_previous_epoch > 0,
        TorchMarketError::NoVolumeInEpoch
    );
    require!(
        user_stats.volume_previous_epoch >= MIN_EPOCH_VOLUME_ELIGIBILITY,
        TorchMarketError::InsufficientVolumeForRewards
    );
    require!(
        protocol_treasury.total_volume_previous_epoch > 0,
        TorchMarketError::NoVolumeInEpoch
    );

    let claim_amount = crate::math::calc_claim_with_cap(
        user_stats.volume_previous_epoch,
        protocol_treasury.distributable_amount,
        protocol_treasury.total_volume_previous_epoch,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        claim_amount >= MIN_CLAIM_AMOUNT,
        TorchMarketError::ClaimBelowMinimum
    );
    Ok(claim_amount)
}

// Apply post-claim bookkeeping (claimable_epoch derived inside).
fn finalize_claim(
    user_stats: &mut UserStats,
    protocol_treasury: &mut ProtocolTreasury,
    claim_amount: u64,
) -> Result<()> {
    let claimable_epoch = protocol_treasury.current_epoch.saturating_sub(1);
    protocol_treasury.current_balance = protocol_treasury
        .current_balance
        .saturating_sub(claim_amount);
    protocol_treasury.distributable_amount = protocol_treasury
        .distributable_amount
        .saturating_sub(claim_amount);
    protocol_treasury.total_distributed = protocol_treasury
        .total_distributed
        .checked_add(claim_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_stats.last_epoch_claimed = claimable_epoch;
    user_stats.total_rewards_claimed = user_stats
        .total_rewards_claimed
        .checked_add(claim_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_stats.volume_previous_epoch = 0;
    Ok(())
}

pub fn claim_protocol_rewards(ctx: Context<ClaimProtocolRewards>) -> Result<()> {
    let claim_amount = compute_claim(
        &mut ctx.accounts.user_stats,
        &ctx.accounts.protocol_treasury,
    )?;

    if claim_amount > 0 {
        **ctx
            .accounts
            .protocol_treasury
            .to_account_info()
            .try_borrow_mut_lamports()? -= claim_amount;
        **ctx
            .accounts
            .user
            .to_account_info()
            .try_borrow_mut_lamports()? += claim_amount;
    }

    finalize_claim(
        &mut ctx.accounts.user_stats,
        &mut ctx.accounts.protocol_treasury,
        claim_amount,
    )
}

pub fn claim_protocol_rewards_via_vault(
    ctx: Context<ClaimProtocolRewardsViaVault>,
) -> Result<()> {
    let claim_amount = compute_claim(
        &mut ctx.accounts.user_stats,
        &ctx.accounts.protocol_treasury,
    )?;

    if claim_amount > 0 {
        let vault_info = ctx.accounts.torch_vault.to_account_info();
        let treasury_info = ctx.accounts.protocol_treasury.to_account_info();
        **treasury_info.try_borrow_mut_lamports()? = treasury_info
            .lamports()
            .checked_sub(claim_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_add(claim_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        let vault = &mut ctx.accounts.torch_vault;
        vault.sol_balance = vault
            .sol_balance
            .checked_add(claim_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(claim_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    finalize_claim(
        &mut ctx.accounts.user_stats,
        &mut ctx.accounts.protocol_treasury,
        claim_amount,
    )
}
