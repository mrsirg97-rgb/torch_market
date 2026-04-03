use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::*;
use crate::errors::TorchMarketError;

// Buy tokens from the bonding curve.
//
// This is the core trading function. When a user buys:
// 1. Protocol takes 0.5% fee from SOL input
// 2. Remaining SOL split: inverse decay from 17.5%→2.5% to treasury as bonding progresses
// 3. Tokens are calculated using constant product formula
// 4. [V36] 100% of tokens go to buyer (vote vault removed)
// 5. If curve SOL reaches target, bonding completes
//
// The bonding curve uses: tokens_out = (virtual_tokens * sol_in) / (virtual_sol + sol_in)
// This creates a smooth price curve where early buyers get more tokens.
pub fn buy(ctx: Context<Buy>, args: BuyArgs) -> Result<()> {
    require!(
        args.sol_amount >= MIN_SOL_AMOUNT,
        TorchMarketError::AmountTooSmall
    );

    if ctx.accounts.torch_vault.is_some() {
        require!(
            ctx.accounts.vault_wallet_link.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    if ctx.accounts.vault_token_account.is_some() {
        require!(
            ctx.accounts.torch_vault.is_some(),
            TorchMarketError::WalletNotLinked
        );
    }

    if let Some(vault) = ctx.accounts.torch_vault.as_ref() {
        require!(
            vault.sol_balance >= args.sol_amount,
            TorchMarketError::InsufficientVaultBalance
        );
    }

    let bonding_curve = &mut ctx.accounts.bonding_curve;
    let user_position = &mut ctx.accounts.user_position;
    let global_config = &ctx.accounts.global_config;

    let protocol_fee_total = args
        .sol_amount
        .checked_mul(global_config.protocol_fee_bps as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let dev_wallet_share = protocol_fee_total
        .checked_mul(DEV_WALLET_SHARE_BPS as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let protocol_fee = protocol_fee_total
        .checked_sub(dev_wallet_share)
        .ok_or(TorchMarketError::MathOverflow)?;
    let token_treasury_fee = args
        .sol_amount
        .checked_mul(TREASURY_FEE_BPS as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sol_after_fees = args
        .sol_amount
        .checked_sub(protocol_fee_total)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_sub(token_treasury_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    let reserves = bonding_curve.real_sol_reserves as u128;
    let target = if bonding_curve.bonding_target == 0 {
        BONDING_TARGET_LAMPORTS as u128
    } else {
        bonding_curve.bonding_target as u128
    };

    let treasury_rate_bps = {
        let rate_range = (TREASURY_SOL_MAX_BPS - TREASURY_SOL_MIN_BPS) as u128;

        let decay = reserves
            .checked_mul(rate_range)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(target)
            .ok_or(TorchMarketError::MathOverflow)?;

        let rate = (TREASURY_SOL_MAX_BPS as u128).saturating_sub(decay);
        rate.max(TREASURY_SOL_MIN_BPS as u128) as u16
    };

    let creator_rate_bps = {
        let rate_range = (CREATOR_SOL_MAX_BPS - CREATOR_SOL_MIN_BPS) as u128;

        let growth = reserves
            .checked_mul(rate_range)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(target)
            .ok_or(TorchMarketError::MathOverflow)?;

        let rate = (CREATOR_SOL_MIN_BPS as u128)
            .checked_add(growth)
            .ok_or(TorchMarketError::MathOverflow)?;
        rate.min(CREATOR_SOL_MAX_BPS as u128) as u16
    };

    let total_split = sol_after_fees
        .checked_mul(treasury_rate_bps as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_community_token = ctx.accounts.token_treasury.total_bought_back == COMMUNITY_TOKEN_SENTINEL;
    let creator_sol = if is_community_token {
        0u64
    } else {
        sol_after_fees
            .checked_mul(creator_rate_bps as u64)
            .ok_or(TorchMarketError::MathOverflow)?
            .checked_div(10000)
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
    let tokens_out = (bonding_curve.virtual_token_reserves as u128)
        .checked_mul(sol_to_curve as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(
            (bonding_curve.virtual_sol_reserves as u128)
                .checked_add(sol_to_curve as u128)
                .ok_or(TorchMarketError::MathOverflow)?,
        )
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    require!(
        tokens_out <= bonding_curve.real_token_reserves,
        TorchMarketError::InsufficientTokens
    );

    let tokens_to_buyer = tokens_out;
    require!(
        tokens_to_buyer >= args.min_tokens_out,
        TorchMarketError::SlippageExceeded
    );

    let token_dest_balance = if let Some(ref vault_ata) = ctx.accounts.vault_token_account {
        vault_ata.amount
    } else {
        ctx.accounts.buyer_token_account.amount
    };

    let dest_new_balance = token_dest_balance
        .checked_add(tokens_to_buyer)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        dest_new_balance <= MAX_WALLET_TOKENS,
        TorchMarketError::MaxWalletExceeded
    );

    let mint_key = ctx.accounts.mint.key();
    let seeds = &[
        BONDING_CURVE_SEED,
        mint_key.as_ref(),
        &[bonding_curve.bump],
    ];
    let signer_seeds = &[&seeds[..]][..];
    let token_destination = if let Some(ref vault_ata) = ctx.accounts.vault_token_account {
        vault_ata.to_account_info()
    } else {
        ctx.accounts.buyer_token_account.to_account_info()
    };

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.token_vault.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: token_destination,
                authority: bonding_curve.to_account_info(),
            },
            signer_seeds,
        ),
        tokens_to_buyer,
        TOKEN_DECIMALS,
    )?;

    if ctx.accounts.torch_vault.is_some() {
        let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
        let bc_info = bonding_curve.to_account_info();
        let tt_info = ctx.accounts.token_treasury.to_account_info();
        let dw_info = ctx.accounts.dev_wallet.to_account_info();
        let pt_info = ctx.accounts.protocol_treasury.to_account_info();
        let cr_info = ctx.accounts.creator.to_account_info();
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_sub(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        **bc_info.try_borrow_mut_lamports()? = bc_info
            .lamports()
            .checked_add(sol_to_curve)
            .ok_or(TorchMarketError::MathOverflow)?;
        **tt_info.try_borrow_mut_lamports()? = tt_info
            .lamports()
            .checked_add(total_to_treasury)
            .ok_or(TorchMarketError::MathOverflow)?;
        **dw_info.try_borrow_mut_lamports()? = dw_info
            .lamports()
            .checked_add(dev_wallet_share)
            .ok_or(TorchMarketError::MathOverflow)?;
        **pt_info.try_borrow_mut_lamports()? = pt_info
            .lamports()
            .checked_add(protocol_fee)
            .ok_or(TorchMarketError::MathOverflow)?;
        **cr_info.try_borrow_mut_lamports()? = cr_info
            .lamports()
            .checked_add(creator_sol)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: bonding_curve.to_account_info(),
                },
            ),
            sol_to_curve,
        )?;

        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.token_treasury.to_account_info(),
                },
            ),
            total_to_treasury,
        )?;

        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.dev_wallet.to_account_info(),
                },
            ),
            dev_wallet_share,
        )?;

        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.protocol_treasury.to_account_info(),
                },
            ),
            protocol_fee,
        )?;

        // [V34] Creator SOL share
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.creator.to_account_info(),
                },
            ),
            creator_sol,
        )?;
    }

    let protocol_treasury = &mut ctx.accounts.protocol_treasury;
    protocol_treasury.total_fees_received = protocol_treasury
        .total_fees_received
        .checked_add(protocol_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    if let Some(vault) = ctx.accounts.torch_vault.as_mut() {
        vault.sol_balance = vault
            .sol_balance
            .checked_sub(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_spent = vault
            .total_spent
            .checked_add(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    bonding_curve.virtual_sol_reserves = bonding_curve
        .virtual_sol_reserves
        .checked_add(sol_to_curve)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_token_reserves = bonding_curve
        .virtual_token_reserves
        .checked_sub(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_sol_reserves = bonding_curve
        .real_sol_reserves
        .checked_add(sol_to_curve)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_token_reserves = bonding_curve
        .real_token_reserves
        .checked_sub(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;

    let token_treasury = &mut ctx.accounts.token_treasury;
    token_treasury.sol_balance = token_treasury
        .sol_balance
        .checked_add(total_to_treasury)
        .ok_or(TorchMarketError::MathOverflow)?;
    let is_first_buy = user_position.user == Pubkey::default();
    if is_first_buy {
        user_position.user = ctx.accounts.buyer.key();
        user_position.bonding_curve = bonding_curve.key();
        user_position.bump = ctx.bumps.user_position;
    }

    let completion_target = if bonding_curve.bonding_target == 0 {
        BONDING_TARGET_LAMPORTS
    } else {
        bonding_curve.bonding_target
    };

    if bonding_curve.real_sol_reserves >= completion_target {
        bonding_curve.bonding_complete = true;
        bonding_curve.bonding_complete_slot = Clock::get()?.slot;
    }

    user_position.total_purchased = user_position
        .total_purchased
        .checked_add(tokens_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_position.tokens_received = user_position
        .tokens_received
        .checked_add(tokens_to_buyer)
        .ok_or(TorchMarketError::MathOverflow)?;
    user_position.total_sol_spent = user_position
        .total_sol_spent
        .checked_add(args.sol_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.last_activity_slot = Clock::get()?.slot;
    if let Some(user_stats) = ctx.accounts.user_stats.as_mut() {
        let protocol_treasury = &ctx.accounts.protocol_treasury;
        if user_stats.user == Pubkey::default() {
            user_stats.user = ctx.accounts.buyer.key();
            user_stats.bump = ctx.bumps.user_stats.expect("user_stats bump should exist");
        }

        user_stats.total_volume = user_stats
            .total_volume
            .checked_add(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        if user_stats.last_volume_epoch < protocol_treasury.current_epoch {
            user_stats.volume_previous_epoch = user_stats.volume_current_epoch;
            user_stats.volume_current_epoch = 0;
        }

        user_stats.volume_current_epoch = user_stats
            .volume_current_epoch
            .checked_add(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
        user_stats.last_volume_epoch = protocol_treasury.current_epoch;
    }

    {
        let protocol_treasury = &mut ctx.accounts.protocol_treasury;
        protocol_treasury.total_volume_current_epoch = protocol_treasury
            .total_volume_current_epoch
            .checked_add(args.sol_amount)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    
    Ok(())
}

// Sell tokens back to the bonding curve.
//
// Users can sell their tokens to receive SOL back:
// 1. User sends tokens to the token vault
// 2. SOL amount is calculated using the bonding curve formula
// 3. Protocol takes 1% fee from SOL output
// 4. Remaining SOL is sent to the seller
//
// Note: No burn on sell - the burn only happens on buy.
// The bonding curve formula ensures price decreases as tokens are sold back.
pub fn sell(ctx: Context<Sell>, args: SellArgs) -> Result<()> {
    require!(args.token_amount > 0, TorchMarketError::ZeroAmount);
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

    let bonding_curve = &mut ctx.accounts.bonding_curve;
    let token_source_balance = if let Some(ref vault_ata) = ctx.accounts.vault_token_account {
        vault_ata.amount
    } else {
        ctx.accounts.seller_token_account.amount
    };
    require!(
        token_source_balance >= args.token_amount,
        TorchMarketError::InsufficientTokens
    );

    let sol_out = (bonding_curve.virtual_sol_reserves as u128)
        .checked_mul(args.token_amount as u128)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(
            (bonding_curve.virtual_token_reserves as u128)
                .checked_add(args.token_amount as u128)
                .ok_or(TorchMarketError::MathOverflow)?,
        )
        .ok_or(TorchMarketError::MathOverflow)? as u64;
    let sell_fee = sol_out
        .checked_mul(SELL_FEE_BPS as u64)
        .ok_or(TorchMarketError::MathOverflow)?
        .checked_div(10000)
        .ok_or(TorchMarketError::MathOverflow)?;
    let sol_to_seller = sol_out
        .checked_sub(sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        bonding_curve.real_sol_reserves >= sol_out,
        TorchMarketError::InsufficientSol
    );

    require!(
        sol_to_seller >= args.min_sol_out,
        TorchMarketError::SlippageExceeded
    );

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
                    to: ctx.accounts.token_vault.to_account_info(),
                    authority: vault.to_account_info(),
                },
                vault_signer_seeds,
            ),
            args.token_amount,
            TOKEN_DECIMALS,
        )?;
    } else {
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
    }

    let bonding_curve_info = bonding_curve.to_account_info();
    if ctx.accounts.torch_vault.is_some() {
        let vault_info = ctx.accounts.torch_vault.as_ref().unwrap().to_account_info();
        **bonding_curve_info.try_borrow_mut_lamports()? = bonding_curve_info
            .lamports()
            .checked_sub(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
        **vault_info.try_borrow_mut_lamports()? = vault_info
            .lamports()
            .checked_add(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
        let vault = ctx.accounts.torch_vault.as_mut().unwrap();
        vault.sol_balance = vault
            .sol_balance
            .checked_add(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
        vault.total_received = vault
            .total_received
            .checked_add(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
    } else {
        let seller_info = ctx.accounts.seller.to_account_info();
        **bonding_curve_info.try_borrow_mut_lamports()? = bonding_curve_info
            .lamports()
            .checked_sub(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
        **seller_info.try_borrow_mut_lamports()? = seller_info
            .lamports()
            .checked_add(sol_to_seller)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    let token_treasury = &mut ctx.accounts.token_treasury;
    let token_treasury_info = token_treasury.to_account_info();
    **bonding_curve_info.try_borrow_mut_lamports()? = bonding_curve_info
        .lamports()
        .checked_sub(sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    **token_treasury_info.try_borrow_mut_lamports()? = token_treasury_info
        .lamports()
        .checked_add(sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    token_treasury.sol_balance = token_treasury
        .sol_balance
        .checked_add(sell_fee)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_sol_reserves = bonding_curve
        .virtual_sol_reserves
        .checked_sub(sol_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.virtual_token_reserves = bonding_curve
        .virtual_token_reserves
        .checked_add(args.token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_sol_reserves = bonding_curve
        .real_sol_reserves
        .checked_sub(sol_out)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.real_token_reserves = bonding_curve
        .real_token_reserves
        .checked_add(args.token_amount)
        .ok_or(TorchMarketError::MathOverflow)?;
    bonding_curve.last_activity_slot = Clock::get()?.slot;
    if let Some(user_stats) = ctx.accounts.user_stats.as_mut() {
        if let Some(protocol_treasury) = ctx.accounts.protocol_treasury.as_ref() {
            user_stats.total_volume = user_stats
                .total_volume
                .checked_add(sol_out)
                .ok_or(TorchMarketError::MathOverflow)?;
            if user_stats.last_volume_epoch < protocol_treasury.current_epoch {
                user_stats.volume_previous_epoch = user_stats.volume_current_epoch;
                user_stats.volume_current_epoch = 0;
            }
            user_stats.volume_current_epoch = user_stats
                .volume_current_epoch
                .checked_add(sol_out)
                .ok_or(TorchMarketError::MathOverflow)?;
            user_stats.last_volume_epoch = protocol_treasury.current_epoch;
        }
    }

    if let Some(protocol_treasury) = ctx.accounts.protocol_treasury.as_mut() {
        protocol_treasury.total_volume_current_epoch = protocol_treasury
            .total_volume_current_epoch
            .checked_add(sol_out)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    Ok(())
}
