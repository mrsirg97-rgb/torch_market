use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::state::ProtocolTreasury;

// Initialize the protocol treasury PDA.
// Called once after V11 deploy by protocol authority.
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

// Advance the protocol treasury epoch.
// Permissionless crank - anyone can call after 7 days.
// Calculates distributable amount (balance above reserve floor).
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
    ctx.accounts.protocol_treasury.distributable_amount = available_balance
        .saturating_sub(reserve_floor);
    ctx.accounts.protocol_treasury.total_volume_current_epoch = 0;
    ctx.accounts.protocol_treasury.current_epoch = ctx.accounts.protocol_treasury
        .current_epoch
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;
    ctx.accounts.protocol_treasury.last_epoch_ts = current_ts;

    Ok(())
}

// Claim protocol rewards based on trading volume.
// Users with >= 2 SOL volume in the previous epoch can claim their proportional share of the distributable amount. Minimum claim: 0.1 SOL.
pub fn claim_protocol_rewards(ctx: Context<ClaimProtocolRewards>) -> Result<()> {
    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    let current_epoch = ctx.accounts.protocol_treasury.current_epoch;
    require!(
        current_epoch > 0,
        TorchMarketError::NoRewardsAvailable
    );

    if ctx.accounts.user_stats.last_volume_epoch < current_epoch
        && ctx.accounts.user_stats.volume_current_epoch > 0
    {
        ctx.accounts.user_stats.volume_previous_epoch = ctx.accounts.user_stats.volume_current_epoch;
        ctx.accounts.user_stats.volume_current_epoch = 0;
        ctx.accounts.user_stats.last_volume_epoch = current_epoch;
    }

    let claimable_epoch = current_epoch.saturating_sub(1);
    require!(
        ctx.accounts.user_stats.last_epoch_claimed < claimable_epoch,
        TorchMarketError::AlreadyClaimed
    );

    require!(
        ctx.accounts.protocol_treasury.distributable_amount > 0,
        TorchMarketError::NoRewardsAvailable
    );

    require!(
        ctx.accounts.user_stats.volume_previous_epoch > 0,
        TorchMarketError::NoVolumeInEpoch
    );

    require!(
        ctx.accounts.user_stats.volume_previous_epoch >= MIN_EPOCH_VOLUME_ELIGIBILITY,
        TorchMarketError::InsufficientVolumeForRewards
    );

    require!(
        ctx.accounts.protocol_treasury.total_volume_previous_epoch > 0,
        TorchMarketError::NoVolumeInEpoch
    );

    let user_share = (ctx.accounts.user_stats.volume_previous_epoch as u128)
        .checked_mul(ctx.accounts.protocol_treasury.distributable_amount as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(ctx.accounts.protocol_treasury.total_volume_previous_epoch as u128)
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let claim_amount = user_share.min(ctx.accounts.protocol_treasury.distributable_amount);
    let max_claim = ctx.accounts.protocol_treasury.distributable_amount
        .checked_mul(MAX_CLAIM_SHARE_BPS)
        .ok_or(TorchMarketError::MathOverflow)?
        / 10_000;
    let claim_amount = claim_amount.min(max_claim);
    require!(
        claim_amount >= MIN_CLAIM_AMOUNT,
        TorchMarketError::ClaimBelowMinimum
    );

    if claim_amount > 0 {
        if ctx.accounts.torch_vault.is_some() {
            let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
            let treasury_info = ctx.accounts.protocol_treasury.to_account_info();
            **treasury_info.try_borrow_mut_lamports()? = treasury_info
                .lamports()
                .checked_sub(claim_amount)
                .ok_or(TorchMarketError::MathOverflow)?;
            **vault_info.try_borrow_mut_lamports()? = vault_info
                .lamports()
                .checked_add(claim_amount)
                .ok_or(TorchMarketError::MathOverflow)?;

            let vault = ctx.accounts.torch_vault.as_mut().unwrap();
            vault.sol_balance = vault
                .sol_balance
                .checked_add(claim_amount)
                .ok_or(TorchMarketError::MathOverflow)?;
            vault.total_received = vault
                .total_received
                .checked_add(claim_amount)
                .ok_or(TorchMarketError::MathOverflow)?;
        } else {
            **ctx.accounts.protocol_treasury.to_account_info().try_borrow_mut_lamports()? -= claim_amount;
            **ctx.accounts.user.to_account_info().try_borrow_mut_lamports()? += claim_amount;
        }
    }

    ctx.accounts.protocol_treasury.current_balance = ctx.accounts.protocol_treasury
        .current_balance
        .saturating_sub(claim_amount);
    ctx.accounts.protocol_treasury.distributable_amount = ctx.accounts.protocol_treasury
        .distributable_amount
        .saturating_sub(claim_amount);
    ctx.accounts.protocol_treasury.total_distributed = ctx.accounts.protocol_treasury
        .total_distributed
        .checked_add(claim_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    ctx.accounts.user_stats.last_epoch_claimed = claimable_epoch;
    ctx.accounts.user_stats.total_rewards_claimed = ctx.accounts.user_stats
        .total_rewards_claimed
        .checked_add(claim_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    ctx.accounts.user_stats.volume_previous_epoch = 0;

    Ok(())
}
