use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;
use crate::math;
use crate::state::{BondingCurve, ProtocolTreasury, Treasury, UserPosition, UserStats};

// ============================================================================
// Buy — shared helpers
// ============================================================================

// Computed SOL split for a buy. All values in lamports.
struct BuyComputed {
    dev_wallet_share: u64,
    protocol_fee: u64,       // protocol_fee_total - dev_wallet_share
    creator_sol: u64,
    sol_to_curve: u64,
    total_to_treasury: u64,  // token_treasury_fee + sol_to_treasury_split
}

// Pure math: compute the 5-way SOL split for a buy. No state mutation.
fn compute_buy_split(
    sol_amount: u64,
    protocol_fee_bps: u16,
    real_sol_reserves: u64,
    bonding_target: u64,
    is_community_token: bool,
) -> Result<BuyComputed> {
    let protocol_fee_total = math::calc_protocol_fee(sol_amount, protocol_fee_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    let dev_wallet_share = math::calc_dev_wallet_share(protocol_fee_total)
        .ok_or(TorchMarketError::MathOverflow)?;
    let protocol_fee = protocol_fee_total
        .checked_sub(dev_wallet_share)
        .ok_or(TorchMarketError::MathOverflow)?;
    let token_treasury_fee =
        math::calc_token_treasury_fee(sol_amount).ok_or(TorchMarketError::MathOverflow)?;
    let sol_after_fees = sol_amount
        .checked_sub(protocol_fee_total)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_sub(token_treasury_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    let target = if bonding_target == 0 {
        BONDING_TARGET_LAMPORTS
    } else {
        bonding_target
    };
    let treasury_rate_bps =
        math::calc_treasury_rate_bps(real_sol_reserves, target).ok_or(TorchMarketError::MathOverflow)?;
    let creator_rate_bps =
        math::calc_creator_rate_bps(real_sol_reserves, target).ok_or(TorchMarketError::MathOverflow)?;
    let total_split = sol_after_fees
        .checked_mul(treasury_rate_bps as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let creator_sol = if is_community_token {
        0
    } else {
        sol_after_fees
            .checked_mul(creator_rate_bps as u64)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(TorchMarketError::MathOverflow)?
    };
    let sol_to_treasury_split = total_split
        .checked_sub(creator_sol)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sol_to_curve = sol_after_fees
        .checked_sub(total_split)
        .ok_or(TorchMarketError::MathOverflow)?;
    let total_to_treasury = token_treasury_fee
        .checked_add(sol_to_treasury_split)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(BuyComputed {
        dev_wallet_share,
        protocol_fee,
        creator_sol,
        sol_to_curve,
        total_to_treasury,
    })
}

// Validate slippage + wallet cap, return the curve-derived token amount.
fn quote_buy_tokens(
    virtual_token_reserves: u64,
    virtual_sol_reserves: u64,
    real_token_reserves: u64,
    sol_to_curve: u64,
    dest_balance: u64,
    min_tokens_out: u64,
) -> Result<u64> {
    let tokens_out =
        math::calc_tokens_out(virtual_token_reserves, virtual_sol_reserves, sol_to_curve)
            .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        tokens_out <= real_token_reserves,
        TorchMarketError::InsufficientTokens
    );
    require!(
        tokens_out >= min_tokens_out,
        TorchMarketError::SlippageExceeded
    );
    let dest_new_balance = dest_balance
        .checked_add(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        dest_new_balance <= MAX_WALLET_TOKENS,
        TorchMarketError::MaxWalletExceeded
    );
    Ok(tokens_out)
}

// Update state after a buy: bonding curve reserves, completion check, user
// position, user stats, protocol treasury epoch volume.
#[allow(clippy::too_many_arguments)]
fn finalize_buy_state(
    bonding_curve: &mut BondingCurve,
    bonding_curve_key: Pubkey,
    token_treasury: &mut Treasury,
    user_position: &mut UserPosition,
    user_stats: Option<&mut UserStats>,
    protocol_treasury: &mut ProtocolTreasury,
    buyer_key: Pubkey,
    user_position_bump: u8,
    user_stats_bump: Option<u8>,
    sol_amount: u64,
    tokens_out: u64,
    computed: &BuyComputed,
) -> Result<()> {
    protocol_treasury.total_fees_received = protocol_treasury
        .total_fees_received
        .checked_add(computed.protocol_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    bonding_curve.virtual_sol_reserves = bonding_curve
        .virtual_sol_reserves
        .checked_add(computed.sol_to_curve)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_token_reserves = bonding_curve
        .virtual_token_reserves
        .checked_sub(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_sol_reserves = bonding_curve
        .real_sol_reserves
        .checked_add(computed.sol_to_curve)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_token_reserves = bonding_curve
        .real_token_reserves
        .checked_sub(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;

    token_treasury.sol_balance = token_treasury
        .sol_balance
        .checked_add(computed.total_to_treasury)
        .ok_or(TorchMarketError::MathOverflow)?;

    let is_first_buy = user_position.user == Pubkey::default();
    if is_first_buy {
        user_position.user = buyer_key;
        user_position.bonding_curve = bonding_curve_key;
        user_position.bump = user_position_bump;
    }

    let completion_target = if bonding_curve.bonding_target == 0 {
        BONDING_TARGET_LAMPORTS
    } else {
        bonding_curve.bonding_target
    };
    let current_slot = Clock::get()?.slot;
    if bonding_curve.real_sol_reserves >= completion_target {
        bonding_curve.bonding_complete = true;
        bonding_curve.bonding_complete_slot = current_slot;
    }

    user_position.total_purchased = user_position
        .total_purchased
        .checked_add(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_position.tokens_received = user_position
        .tokens_received
        .checked_add(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_position.total_sol_spent = user_position
        .total_sol_spent
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.last_activity_slot = current_slot;

    if let Some(stats) = user_stats {
        if stats.user == Pubkey::default() {
            stats.user = buyer_key;
            stats.bump = user_stats_bump
                .expect("user_stats bump must be Some when user_stats account is provided");
        }
        stats.total_volume = stats
            .total_volume
            .checked_add(sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        if stats.last_volume_epoch < protocol_treasury.current_epoch {
            stats.volume_previous_epoch = stats.volume_current_epoch;
            stats.volume_current_epoch = 0;
        }
        stats.volume_current_epoch = stats
            .volume_current_epoch
            .checked_add(sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        stats.last_volume_epoch = protocol_treasury.current_epoch;
    }

    protocol_treasury.total_volume_current_epoch = protocol_treasury
        .total_volume_current_epoch
        .checked_add(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    Ok(())
}

// ============================================================================
// Buy — handlers
// ============================================================================

// Buy tokens from the bonding curve, funded by the buyer's wallet.
pub fn buy(ctx: Context<Buy>, args: BuyArgs) -> Result<()> {
    let computed = compute_buy_split(
        args.sol_amount,
        ctx.accounts.global_config.protocol_fee_bps,
        ctx.accounts.bonding_curve.real_sol_reserves,
        ctx.accounts.bonding_curve.bonding_target,
        ctx.accounts.token_treasury.is_community_token,
    )?;

    let tokens_out = quote_buy_tokens(
        ctx.accounts.bonding_curve.virtual_token_reserves,
        ctx.accounts.bonding_curve.virtual_sol_reserves,
        ctx.accounts.bonding_curve.real_token_reserves,
        computed.sol_to_curve,
        ctx.accounts.buyer_token_account.amount,
        args.min_tokens_out,
    )?;

    let mint_key = ctx.accounts.mint.key();
    let bc_bump = ctx.accounts.bonding_curve.bump;
    let seeds = &[BONDING_CURVE_SEED, mint_key.as_ref(), &[bc_bump]];
    let signer_seeds = &[&seeds[..]][..];
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.token_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.buyer_token_account.to_account_info(),
                authority: ctx.accounts.bonding_curve.to_account_info(),
            },
            signer_seeds,
        ),
        tokens_out,
        TOKEN_DECIMALS,
    )?;

    let sys_program = ctx.accounts.system_program.to_account_info();
    let buyer_info = ctx.accounts.buyer.to_account_info();
    distribute_buy_sol_from_signer(
        &sys_program,
        &buyer_info,
        &ctx.accounts.bonding_curve.to_account_info(),
        &ctx.accounts.token_treasury.to_account_info(),
        &ctx.accounts.dev_wallet.to_account_info(),
        &ctx.accounts.protocol_treasury.to_account_info(),
        &ctx.accounts.creator,
        &computed,
    )?;

    let buyer_key = ctx.accounts.buyer.key();
    let bonding_curve_key = ctx.accounts.bonding_curve.key();
    let user_position_bump = ctx.bumps.user_position;
    let user_stats_bump = ctx.bumps.user_stats;
    let user_stats_ref: Option<&mut UserStats> = ctx
        .accounts
        .user_stats
        .as_deref_mut()
        .map(|a| &mut **a);

    finalize_buy_state(
        &mut ctx.accounts.bonding_curve,
        bonding_curve_key,
        &mut ctx.accounts.token_treasury,
        &mut ctx.accounts.user_position,
        user_stats_ref,
        &mut ctx.accounts.protocol_treasury,
        buyer_key,
        user_position_bump,
        user_stats_bump,
        args.sol_amount,
        tokens_out,
        &computed,
    )
}

// Vault-routed buy: vault funds the SOL, tokens deposited into vault ATA.
pub fn buy_via_vault(ctx: Context<BuyViaVault>, args: BuyArgs) -> Result<()> {
    require!(
        ctx.accounts.torch_vault.sol_balance >= args.sol_amount,
        TorchMarketError::InsufficientVaultBalance
    );

    let computed = compute_buy_split(
        args.sol_amount,
        ctx.accounts.global_config.protocol_fee_bps,
        ctx.accounts.bonding_curve.real_sol_reserves,
        ctx.accounts.bonding_curve.bonding_target,
        ctx.accounts.token_treasury.is_community_token,
    )?;

    let tokens_out = quote_buy_tokens(
        ctx.accounts.bonding_curve.virtual_token_reserves,
        ctx.accounts.bonding_curve.virtual_sol_reserves,
        ctx.accounts.bonding_curve.real_token_reserves,
        computed.sol_to_curve,
        ctx.accounts.vault_token_account.amount,
        args.min_tokens_out,
    )?;

    let mint_key = ctx.accounts.mint.key();
    let bc_bump = ctx.accounts.bonding_curve.bump;
    let seeds = &[BONDING_CURVE_SEED, mint_key.as_ref(), &[bc_bump]];
    let signer_seeds = &[&seeds[..]][..];
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.token_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.vault_token_account.to_account_info(),
                authority: ctx.accounts.bonding_curve.to_account_info(),
            },
            signer_seeds,
        ),
        tokens_out,
        TOKEN_DECIMALS,
    )?;

    distribute_buy_sol_from_vault(
        &ctx.accounts.torch_vault.to_account_info(),
        &ctx.accounts.bonding_curve.to_account_info(),
        &ctx.accounts.token_treasury.to_account_info(),
        &ctx.accounts.dev_wallet.to_account_info(),
        &ctx.accounts.protocol_treasury.to_account_info(),
        &ctx.accounts.creator,
        args.sol_amount,
        &computed,
    )?;

    let vault = &mut ctx.accounts.torch_vault;
    vault.sol_balance = vault
        .sol_balance
        .checked_sub(args.sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_spent = vault
        .total_spent
        .checked_add(args.sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;

    let buyer_key = ctx.accounts.buyer.key();
    let bonding_curve_key = ctx.accounts.bonding_curve.key();
    let user_position_bump = ctx.bumps.user_position;
    let user_stats_bump = ctx.bumps.user_stats;
    let user_stats_ref: Option<&mut UserStats> = ctx
        .accounts
        .user_stats
        .as_deref_mut()
        .map(|a| &mut **a);

    finalize_buy_state(
        &mut ctx.accounts.bonding_curve,
        bonding_curve_key,
        &mut ctx.accounts.token_treasury,
        &mut ctx.accounts.user_position,
        user_stats_ref,
        &mut ctx.accounts.protocol_treasury,
        buyer_key,
        user_position_bump,
        user_stats_bump,
        args.sol_amount,
        tokens_out,
        &computed,
    )
}

// 5-way System.transfer fan-out from the wallet signer.
#[allow(clippy::too_many_arguments)]
fn distribute_buy_sol_from_signer<'info>(
    system_program: &AccountInfo<'info>,
    signer: &AccountInfo<'info>,
    bonding_curve: &AccountInfo<'info>,
    token_treasury: &AccountInfo<'info>,
    dev_wallet: &AccountInfo<'info>,
    protocol_treasury: &AccountInfo<'info>,
    creator: &AccountInfo<'info>,
    computed: &BuyComputed,
) -> Result<()> {
    let xfer = |to: &AccountInfo<'info>, amount: u64| -> Result<()> {
        anchor_lang::system_program::transfer(
            CpiContext::new(
                system_program.clone(),
                anchor_lang::system_program::Transfer {
                    from: signer.clone(),
                    to: to.clone(),
                },
            ),
            amount,
        )
    };
    xfer(bonding_curve, computed.sol_to_curve)?;
    xfer(token_treasury, computed.total_to_treasury)?;
    xfer(dev_wallet, computed.dev_wallet_share)?;
    xfer(protocol_treasury, computed.protocol_fee)?;
    xfer(creator, computed.creator_sol)?;
    Ok(())
}

// 5-way direct-lamport distribution from a program-owned vault PDA.
// Vault PDA can't be a System.transfer source (program-owned), so we shift
// lamports directly. Atomic with the rest of the instruction.
#[allow(clippy::too_many_arguments)]
fn distribute_buy_sol_from_vault<'info>(
    vault: &AccountInfo<'info>,
    bonding_curve: &AccountInfo<'info>,
    token_treasury: &AccountInfo<'info>,
    dev_wallet: &AccountInfo<'info>,
    protocol_treasury: &AccountInfo<'info>,
    creator: &AccountInfo<'info>,
    sol_amount: u64,
    computed: &BuyComputed,
) -> Result<()> {
    **vault.try_borrow_mut_lamports()? = vault
        .lamports()
        .checked_sub(sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    let credit = |to: &AccountInfo<'info>, amount: u64| -> Result<()> {
        **to.try_borrow_mut_lamports()? = to
            .lamports()
            .checked_add(amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        Ok(())
    };
    credit(bonding_curve, computed.sol_to_curve)?;
    credit(token_treasury, computed.total_to_treasury)?;
    credit(dev_wallet, computed.dev_wallet_share)?;
    credit(protocol_treasury, computed.protocol_fee)?;
    credit(creator, computed.creator_sol)?;
    Ok(())
}

// ============================================================================
// Sell — shared helpers
// ============================================================================

struct SellComputed {
    sol_out: u64,
    sell_fee: u64,
    sol_to_seller: u64,
}

fn compute_sell(
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
    real_sol_reserves: u64,
    token_amount: u64,
    min_sol_out: u64,
) -> Result<SellComputed> {
    let sol_out = math::calc_sol_out(virtual_sol_reserves, virtual_token_reserves, token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sell_fee = sol_out
        .checked_mul(SELL_FEE_BPS as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sol_to_seller = sol_out
        .checked_sub(sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        real_sol_reserves >= sol_out,
        TorchMarketError::InsufficientSol
    );
    require!(
        sol_to_seller >= min_sol_out,
        TorchMarketError::SlippageExceeded
    );
    Ok(SellComputed {
        sol_out,
        sell_fee,
        sol_to_seller,
    })
}

#[allow(clippy::too_many_arguments)]
fn finalize_sell_state(
    bonding_curve: &mut BondingCurve,
    token_treasury: &mut Treasury,
    user_stats: Option<&mut UserStats>,
    protocol_treasury: Option<&mut ProtocolTreasury>,
    token_amount: u64,
    computed: &SellComputed,
) -> Result<()> {
    token_treasury.sol_balance = token_treasury
        .sol_balance
        .checked_add(computed.sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_sol_reserves = bonding_curve
        .virtual_sol_reserves
        .checked_sub(computed.sol_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_token_reserves = bonding_curve
        .virtual_token_reserves
        .checked_add(token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_sol_reserves = bonding_curve
        .real_sol_reserves
        .checked_sub(computed.sol_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_token_reserves = bonding_curve
        .real_token_reserves
        .checked_add(token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.last_activity_slot = Clock::get()?.slot;

    if let (Some(stats), Some(pt)) = (user_stats, protocol_treasury) {
        stats.total_volume = stats
            .total_volume
            .checked_add(computed.sol_out)
            .ok_or(TorchMarketError::MathOverflow)?;
        if stats.last_volume_epoch < pt.current_epoch {
            stats.volume_previous_epoch = stats.volume_current_epoch;
            stats.volume_current_epoch = 0;
        }
        stats.volume_current_epoch = stats
            .volume_current_epoch
            .checked_add(computed.sol_out)
            .ok_or(TorchMarketError::MathOverflow)?;
        stats.last_volume_epoch = pt.current_epoch;
        pt.total_volume_current_epoch = pt
            .total_volume_current_epoch
            .checked_add(computed.sol_out)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    Ok(())
}

// Direct lamport shift from bonding_curve to a recipient. Used because the
// curve PDA is program-owned (can't be a System.transfer source) and sells
// pull SOL out of the curve.
fn shift_curve_lamports<'info>(
    bonding_curve: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    **bonding_curve.try_borrow_mut_lamports()? = bonding_curve
        .lamports()
        .checked_sub(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    **to.try_borrow_mut_lamports()? = to
        .lamports()
        .checked_add(amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    Ok(())
}

// ============================================================================
// Sell — handlers
// ============================================================================

// Sell tokens back to the bonding curve; SOL returns to seller's wallet.
pub fn sell(ctx: Context<Sell>, args: SellArgs) -> Result<()> {
    require!(
        ctx.accounts.seller_token_account.amount >= args.token_amount,
        TorchMarketError::InsufficientTokens
    );

    let computed = compute_sell(
        ctx.accounts.bonding_curve.virtual_sol_reserves,
        ctx.accounts.bonding_curve.virtual_token_reserves,
        ctx.accounts.bonding_curve.real_sol_reserves,
        args.token_amount,
        args.min_sol_out,
    )?;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.seller_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.token_vault.to_account_info(),
                authority: ctx.accounts.seller.to_account_info(),
            },
        ),
        args.token_amount,
        TOKEN_DECIMALS,
    )?;

    let bc_info = ctx.accounts.bonding_curve.to_account_info();
    shift_curve_lamports(
        &bc_info,
        &ctx.accounts.seller.to_account_info(),
        computed.sol_to_seller,
    )?;
    shift_curve_lamports(
        &bc_info,
        &ctx.accounts.token_treasury.to_account_info(),
        computed.sell_fee,
    )?;

    let user_stats_ref: Option<&mut UserStats> = ctx
        .accounts
        .user_stats
        .as_deref_mut()
        .map(|a| &mut **a);
    let protocol_treasury_ref: Option<&mut ProtocolTreasury> = ctx
        .accounts
        .protocol_treasury
        .as_deref_mut()
        .map(|a| &mut **a);

    finalize_sell_state(
        &mut ctx.accounts.bonding_curve,
        &mut ctx.accounts.token_treasury,
        user_stats_ref,
        protocol_treasury_ref,
        args.token_amount,
        &computed,
    )
}

// Vault-routed sell: tokens come from vault ATA, SOL proceeds go to vault.
pub fn sell_via_vault(ctx: Context<SellViaVault>, args: SellArgs) -> Result<()> {
    require!(
        ctx.accounts.vault_token_account.amount >= args.token_amount,
        TorchMarketError::InsufficientTokens
    );

    let computed = compute_sell(
        ctx.accounts.bonding_curve.virtual_sol_reserves,
        ctx.accounts.bonding_curve.virtual_token_reserves,
        ctx.accounts.bonding_curve.real_sol_reserves,
        args.token_amount,
        args.min_sol_out,
    )?;

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
                to: ctx.accounts.token_vault.to_account_info(),
                authority: ctx.accounts.torch_vault.to_account_info(),
            },
            vault_signer_seeds,
        ),
        args.token_amount,
        TOKEN_DECIMALS,
    )?;

    let bc_info = ctx.accounts.bonding_curve.to_account_info();
    shift_curve_lamports(
        &bc_info,
        &ctx.accounts.torch_vault.to_account_info(),
        computed.sol_to_seller,
    )?;
    shift_curve_lamports(
        &bc_info,
        &ctx.accounts.token_treasury.to_account_info(),
        computed.sell_fee,
    )?;

    let vault = &mut ctx.accounts.torch_vault;
    vault.sol_balance = vault
        .sol_balance
        .checked_add(computed.sol_to_seller)
        .ok_or(TorchMarketError::MathOverflow)?;
    vault.total_received = vault
        .total_received
        .checked_add(computed.sol_to_seller)
        .ok_or(TorchMarketError::MathOverflow)?;

    let user_stats_ref: Option<&mut UserStats> = ctx
        .accounts
        .user_stats
        .as_deref_mut()
        .map(|a| &mut **a);
    let protocol_treasury_ref: Option<&mut ProtocolTreasury> = ctx
        .accounts
        .protocol_treasury
        .as_deref_mut()
        .map(|a| &mut **a);

    finalize_sell_state(
        &mut ctx.accounts.bonding_curve,
        &mut ctx.accounts.token_treasury,
        user_stats_ref,
        protocol_treasury_ref,
        args.token_amount,
        &computed,
    )
}
