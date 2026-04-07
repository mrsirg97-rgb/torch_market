use anchor_lang::prelude::*;

pub mod constants;
pub mod contexts;
pub mod errors;
pub mod handlers;
pub mod migration;
pub mod pool_validation;
pub mod state;
pub mod token_2022_utils;

#[cfg(kani)]
mod kani_proofs;

use contexts::*;

declare_id!("8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT");

#[program]
pub mod torch_market {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        handlers::admin::initialize(ctx)
    }

    pub fn update_dev_wallet(ctx: Context<UpdateDevWallet>) -> Result<()> {
        handlers::admin::update_dev_wallet(ctx)
    }

    pub fn create_token(ctx: Context<CreateToken2022>, args: CreateTokenArgs) -> Result<()> {
        handlers::token::create_token(ctx, args)
    }

    pub fn buy(ctx: Context<Buy>, args: BuyArgs) -> Result<()> {
        handlers::market::buy(ctx, args)
    }

    pub fn sell(ctx: Context<Sell>, args: SellArgs) -> Result<()> {
        handlers::market::sell(ctx, args)
    }

    pub fn reclaim_failed_token(ctx: Context<ReclaimFailedToken>) -> Result<()> {
        handlers::reclaim::reclaim_failed_token(ctx)
    }

    pub fn contribute_revival(ctx: Context<ContributeRevival>, sol_amount: u64) -> Result<()> {
        handlers::revival::contribute_revival(ctx, sol_amount)
    }

    pub fn fund_migration_sol(ctx: Context<FundMigrationSol>) -> Result<()> {
        handlers::migration::fund_migration_sol(ctx)
    }

    pub fn migrate_to_dex(ctx: Context<MigrateToDex>) -> Result<()> {
        handlers::migration::migrate_to_dex(ctx)
    }

    pub fn harvest_fees<'info>(
        ctx: Context<'_, '_, 'info, 'info, HarvestFees<'info>>,
    ) -> Result<()> {
        handlers::treasury::harvest_fees(ctx)
    }

    pub fn swap_fees_to_sol(ctx: Context<SwapFeesToSol>, minimum_amount_out: u64) -> Result<()> {
        handlers::treasury::swap_fees_to_sol(ctx, minimum_amount_out)
    }

    pub fn star_token(ctx: Context<StarToken>) -> Result<()> {
        handlers::rewards::star_token(ctx)
    }

    pub fn initialize_protocol_treasury(ctx: Context<InitializeProtocolTreasury>) -> Result<()> {
        handlers::protocol_treasury::initialize_protocol_treasury(ctx)
    }

    pub fn advance_protocol_epoch(ctx: Context<AdvanceProtocolEpoch>) -> Result<()> {
        handlers::protocol_treasury::advance_protocol_epoch(ctx)
    }

    pub fn claim_protocol_rewards(ctx: Context<ClaimProtocolRewards>) -> Result<()> {
        handlers::protocol_treasury::claim_protocol_rewards(ctx)
    }

    pub fn borrow(ctx: Context<Borrow>, args: BorrowArgs) -> Result<()> {
        handlers::lending::borrow(ctx, args)
    }

    pub fn repay(ctx: Context<Repay>, sol_amount: u64) -> Result<()> {
        handlers::lending::repay(ctx, sol_amount)
    }

    pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {
        handlers::lending::liquidate(ctx)
    }

    pub fn create_vault(ctx: Context<CreateVault>) -> Result<()> {
        handlers::vault::create_vault(ctx)
    }

    pub fn deposit_vault(ctx: Context<DepositVault>, sol_amount: u64) -> Result<()> {
        handlers::vault::deposit_vault(ctx, sol_amount)
    }

    pub fn withdraw_vault(ctx: Context<WithdrawVault>, sol_amount: u64) -> Result<()> {
        handlers::vault::withdraw_vault(ctx, sol_amount)
    }

    pub fn link_wallet(ctx: Context<LinkWallet>) -> Result<()> {
        handlers::vault::link_wallet(ctx)
    }

    pub fn unlink_wallet(ctx: Context<UnlinkWallet>) -> Result<()> {
        handlers::vault::unlink_wallet(ctx)
    }

    pub fn transfer_authority(ctx: Context<TransferVaultAuthority>) -> Result<()> {
        handlers::vault::transfer_authority(ctx)
    }

    pub fn withdraw_tokens(ctx: Context<WithdrawTokens>, amount: u64) -> Result<()> {
        handlers::vault::withdraw_tokens(ctx, amount)
    }

    pub fn vault_swap(
        ctx: Context<VaultSwap>,
        amount_in: u64,
        minimum_amount_out: u64,
        is_buy: bool,
    ) -> Result<()> {
        handlers::swap::vault_swap(ctx, amount_in, minimum_amount_out, is_buy)
    }

    pub fn enable_short_selling(ctx: Context<EnableShortSelling>) -> Result<()> {
        handlers::short::enable_short_selling(ctx)
    }

    pub fn open_short(ctx: Context<OpenShort>, args: OpenShortArgs) -> Result<()> {
        handlers::short::open_short(ctx, args)
    }

    pub fn close_short(ctx: Context<CloseShort>, token_amount: u64) -> Result<()> {
        handlers::short::close_short(ctx, token_amount)
    }

    pub fn liquidate_short(ctx: Context<LiquidateShort>) -> Result<()> {
        handlers::short::liquidate_short(ctx)
    }

}
