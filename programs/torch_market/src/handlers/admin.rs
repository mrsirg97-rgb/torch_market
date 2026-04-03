use anchor_lang::prelude::*;

use crate::constants::*;
use crate::contexts::*;

pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
    let global_config = &mut ctx.accounts.global_config;

    global_config.authority = ctx.accounts.authority.key();
    global_config.treasury = ctx.accounts.treasury.key();
    global_config.dev_wallet = ctx.accounts.dev_wallet.key();
    global_config._deprecated_platform_treasury = Pubkey::default();
    global_config.protocol_fee_bps = PROTOCOL_FEE_BPS;
    global_config.paused = false;
    global_config.total_tokens_launched = 0;
    global_config.total_volume_sol = 0;
    global_config.bump = ctx.bumps.global_config;

    Ok(())
}

pub fn update_dev_wallet(ctx: Context<UpdateDevWallet>) -> Result<()> {
    let global_config = &mut ctx.accounts.global_config;
    let new_dev_wallet = ctx.accounts.new_dev_wallet.key();

    global_config.dev_wallet = new_dev_wallet;

    Ok(())
}
