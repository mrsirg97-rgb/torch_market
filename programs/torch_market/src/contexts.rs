
use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{Mint as MintInterface, TokenAccount as TokenAccountInterface, TokenInterface},
};

use crate::constants::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::{derive_deep_pool, derive_deep_pool_vault, derive_deep_pool_lp_mint, derive_torch_config};
use crate::state::*;
use crate::token_2022_utils::TOKEN_2022_PROGRAM_ID;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct CreateTokenArgs {
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub sol_target: u64,
    pub community_token: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct BuyArgs {
    pub sol_amount: u64,
    pub min_tokens_out: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SellArgs {
    pub token_amount: u64,
    pub min_sol_out: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct BorrowArgs {
    pub collateral_amount: u64,
    pub sol_to_borrow: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct OpenShortArgs {
    pub sol_collateral: u64,
    pub tokens_to_borrow: u64,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = GlobalConfig::LEN,
        seeds = [GLOBAL_CONFIG_SEED],
        bump
    )]
    pub global_config: Account<'info, GlobalConfig>,
    /// CHECK: Treasury wallet (protocol fees)
    pub treasury: UncheckedAccount<'info>,
    /// CHECK: Dev wallet (25% of treasury fee) [V8]
    pub dev_wallet: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateDevWallet<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        has_one = authority @ TorchMarketError::Unauthorized
    )]
    pub global_config: Account<'info, GlobalConfig>,
    /// CHECK: New dev wallet address
    pub new_dev_wallet: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(args: CreateTokenArgs)]
pub struct CreateToken2022<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        constraint = !global_config.paused @ TorchMarketError::ProtocolPaused
    )]
    pub global_config: Account<'info, GlobalConfig>,
    /// CHECK: Token-2022 mint - initialized manually
    #[account(mut, signer)]
    pub mint: AccountInfo<'info>,
    #[account(
        init,
        payer = creator,
        space = BondingCurve::LEN,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    /// CHECK: Token-2022 ATA for bonding curve - created via CPI
    #[account(mut)]
    pub token_vault: AccountInfo<'info>,
    #[account(
        init,
        payer = creator,
        space = Treasury::LEN,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump
    )]
    pub treasury: Account<'info, Treasury>,
    /// CHECK: Treasury's Token-2022 ATA - holds vote vault tokens during bonding,
    #[account(mut)]
    pub treasury_token_account: AccountInfo<'info>,
    #[account(
        init,
        payer = creator,
        space = TreasuryLock::LEN,
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump
    )]
    pub treasury_lock: Account<'info, TreasuryLock>,
    /// CHECK: Treasury lock's Token-2022 ATA — holds 250M locked tokens.
    #[account(mut)]
    pub treasury_lock_token_account: AccountInfo<'info>,
    /// CHECK: Token-2022 program
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Buy<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        constraint = !global_config.paused @ TorchMarketError::ProtocolPaused
    )]
    pub global_config: Box<Account<'info, GlobalConfig>>,
    /// CHECK: Dev wallet receives 25% of protocol fee [V8]
    #[account(
        mut,
        constraint = dev_wallet.key() == global_config.dev_wallet @ TorchMarketError::InvalidDevWallet
    )]
    pub dev_wallet: UncheckedAccount<'info>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = !bonding_curve.bonding_complete @ TorchMarketError::BondingComplete
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = bonding_curve,
        associated_token::token_program = token_program,
    )]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = bonding_curve.treasury_bump,
    )]
    pub token_treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = token_treasury,
        associated_token::token_program = token_program,
    )]
    pub treasury_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init_if_needed,
        payer = buyer,
        associated_token::mint = mint,
        associated_token::authority = buyer,
        associated_token::token_program = token_program,
    )]
    pub buyer_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init_if_needed,
        payer = buyer,
        space = UserPosition::LEN,
        seeds = [USER_POSITION_SEED, bonding_curve.key().as_ref(), buyer.key().as_ref()],
        bump
    )]
    pub user_position: Box<Account<'info, UserPosition>>,
    #[account(
        init_if_needed,
        payer = buyer,
        space = UserStats::LEN,
        seeds = [USER_STATS_SEED, buyer.key().as_ref()],
        bump
    )]
    pub user_stats: Option<Box<Account<'info, UserStats>>>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump,
    )]
    pub protocol_treasury: Box<Account<'info, ProtocolTreasury>>,
    /// CHECK: Validated against bonding_curve.creator
    #[account(
        mut,
        constraint = creator.key() == bonding_curve.creator @ TorchMarketError::InvalidAuthority
    )]
    pub creator: AccountInfo<'info>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, buyer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault.as_ref().unwrap(),
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Sell<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = !bonding_curve.bonding_complete @ TorchMarketError::BondingComplete
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = bonding_curve,
        associated_token::token_program = token_program,
    )]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = seller,
        associated_token::token_program = token_program,
    )]
    pub seller_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        seeds = [USER_POSITION_SEED, bonding_curve.key().as_ref(), seller.key().as_ref()],
        bump = user_position.bump
    )]
    pub user_position: Option<Box<Account<'info, UserPosition>>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = bonding_curve.treasury_bump,
    )]
    pub token_treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [USER_STATS_SEED, seller.key().as_ref()],
        bump = user_stats.bump,
    )]
    pub user_stats: Option<Box<Account<'info, UserStats>>>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump,
    )]
    pub protocol_treasury: Option<Box<Account<'info, ProtocolTreasury>>>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, seller.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault.as_ref().unwrap(),
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct HarvestFees<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.is_token_2022 @ TorchMarketError::NotToken2022,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = bonding_curve.treasury_bump,
    )]
    pub token_treasury: Account<'info, Treasury>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = token_treasury,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    pub token_2022_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

#[derive(Accounts)]
pub struct SwapFeesToSol<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.migrated @ TorchMarketError::NotMigrated,
        constraint = bonding_curve.is_token_2022 @ TorchMarketError::NotToken2022,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    /// CHECK: Validated against bonding_curve.creator
    #[account(
        mut,
        constraint = creator.key() == bonding_curve.creator @ TorchMarketError::InvalidAuthority
    )]
    pub creator: AccountInfo<'info>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Token-2022 program for project tokens
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ReclaimFailedToken<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = !bonding_curve.bonding_complete @ TorchMarketError::BondingComplete,
        constraint = !bonding_curve.reclaimed @ TorchMarketError::AlreadyReclaimed,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = bonding_curve.treasury_bump,
    )]
    pub token_treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump = protocol_treasury.bump,
    )]
    pub protocol_treasury: Box<Account<'info, ProtocolTreasury>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ContributeRevival<'info> {
    #[account(mut)]
    pub contributor: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.reclaimed @ TorchMarketError::TokenNotReclaimed,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct StarToken<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = user.key() != bonding_curve.creator @ TorchMarketError::CannotStarSelf,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = token_treasury.bump,
    )]
    pub token_treasury: Box<Account<'info, Treasury>>,
    /// CHECK: Creator wallet - receives auto-payout when threshold reached
    #[account(
        mut,
        constraint = creator.key() == bonding_curve.creator @ TorchMarketError::InvalidAuthority
    )]
    pub creator: UncheckedAccount<'info>,
    #[account(
        init,
        payer = user,
        space = StarRecord::LEN,
        seeds = [STAR_RECORD_SEED, user.key().as_ref(), mint.key().as_ref()],
        bump
    )]
    pub star_record: Account<'info, StarRecord>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, user.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializeProtocolTreasury<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        has_one = authority @ TorchMarketError::Unauthorized,
    )]
    pub global_config: Account<'info, GlobalConfig>,
    #[account(
        init,
        payer = authority,
        space = ProtocolTreasury::LEN,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump
    )]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdvanceProtocolEpoch<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump = protocol_treasury.bump,
    )]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
}

#[derive(Accounts)]
pub struct ClaimProtocolRewards<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        seeds = [USER_STATS_SEED, user.key().as_ref()],
        bump = user_stats.bump,
    )]
    pub user_stats: Account<'info, UserStats>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump = protocol_treasury.bump,
    )]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub system_program: Program<'info, System>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, user.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
}

#[derive(Accounts)]
pub struct FundMigrationSol<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.bonding_complete @ TorchMarketError::BondingNotComplete,
        constraint = bonding_curve.vote_finalized @ TorchMarketError::VoteNotFinalized,
        constraint = !bonding_curve.migrated @ TorchMarketError::AlreadyMigrated,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
}

#[derive(Accounts)]
pub struct MigrateToDex<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
    )]
    pub global_config: Box<Account<'info, GlobalConfig>>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.bonding_complete @ TorchMarketError::BondingNotComplete,
        constraint = bonding_curve.vote_finalized @ TorchMarketError::VoteNotFinalized,
        constraint = !bonding_curve.migrated @ TorchMarketError::AlreadyMigrated,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        constraint = treasury.sol_balance >= MIN_MIGRATION_SOL @ TorchMarketError::InsufficientMigrationFee,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = bonding_curve,
        associated_token::token_program = token_2022_program,
    )]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: ATA address validated manually in handler to reduce stack pressure.
    #[account(mut)]
    pub treasury_lock_token_account: AccountInfo<'info>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    /// CHECK: Token-2022 ATA for payer — receives tokens from bonding curve, deposits to DeepPool
    #[account(mut)]
    pub payer_token: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: Torch config PDA — signer namespace for DeepPool pool creation
    #[account(address = derive_torch_config() @ TorchMarketError::InvalidPoolAccount)]
    pub torch_config: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA — PDA = ["deep_pool", torch_config, mint]
    #[account(mut, address = derive_deep_pool(&torch_config.key(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault PDA — will be initialized by create_pool CPI
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool LP mint PDA — will be initialized by create_pool CPI
    #[account(mut, address = derive_deep_pool_lp_mint(&deep_pool.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_lp_mint: AccountInfo<'info>,
    /// CHECK: Payer's LP ATA — receives LP tokens from create_pool, then burned
    #[account(mut)]
    pub payer_lp_account: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA's LP ATA — receives locked LP from create_pool
    #[account(mut)]
    pub deep_pool_lp_account: AccountInfo<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: Token-2022 program for project tokens
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Borrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.migrated @ TorchMarketError::LendingRequiresMigration,
        constraint = !bonding_curve.reclaimed @ TorchMarketError::AlreadyReclaimed,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        constraint = treasury.lending_enabled @ TorchMarketError::LendingNotEnabled,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        init_if_needed,
        payer = borrower,
        seeds = [COLLATERAL_VAULT_SEED, mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = treasury,
        token::token_program = token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = borrower,
        associated_token::token_program = token_program,
    )]
    pub borrower_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init_if_needed,
        payer = borrower,
        space = LoanPosition::LEN,
        seeds = [LOAN_SEED, mint.key().as_ref(), borrower.key().as_ref()],
        bump
    )]
    pub loan_position: Box<Account<'info, LoanPosition>>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, borrower.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault.as_ref().unwrap(),
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [COLLATERAL_VAULT_SEED, mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = treasury,
        token::token_program = token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = borrower,
        associated_token::token_program = token_program,
    )]
    pub borrower_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [LOAN_SEED, mint.key().as_ref(), borrower.key().as_ref()],
        bump = loan_position.bump,
        constraint = loan_position.borrowed_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub loan_position: Box<Account<'info, LoanPosition>>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, borrower.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault.as_ref().unwrap(),
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    /// CHECK: Borrower wallet - receives rent if position is closed
    #[account(mut)]
    pub borrower: AccountInfo<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [COLLATERAL_VAULT_SEED, mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = treasury,
        token::token_program = token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init_if_needed,
        payer = liquidator,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [LOAN_SEED, mint.key().as_ref(), borrower.key().as_ref()],
        bump = loan_position.bump,
        constraint = loan_position.borrowed_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub loan_position: Box<Account<'info, LoanPosition>>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, liquidator.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.as_ref().unwrap().key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault.as_ref().unwrap(),
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
}

#[derive(Accounts)]
pub struct CreateVault<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        init,
        payer = creator,
        space = TorchVault::LEN,
        seeds = [TORCH_VAULT_SEED, creator.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TorchVault>,
    #[account(
        init,
        payer = creator,
        space = VaultWalletLink::LEN,
        seeds = [VAULT_WALLET_LINK_SEED, creator.key().as_ref()],
        bump
    )]
    pub wallet_link: Account<'info, VaultWalletLink>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositVault<'info> {
    #[account(mut)]
    pub depositor: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, TorchVault>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct WithdrawVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
        has_one = authority @ TorchMarketError::VaultUnauthorized,
    )]
    pub vault: Account<'info, TorchVault>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LinkWallet<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
        has_one = authority @ TorchMarketError::VaultUnauthorized,
    )]
    pub vault: Account<'info, TorchVault>,
    /// CHECK: The wallet to link (doesn't need to sign — authority controls this)
    pub wallet_to_link: UncheckedAccount<'info>,
    #[account(
        init,
        payer = authority,
        space = VaultWalletLink::LEN,
        seeds = [VAULT_WALLET_LINK_SEED, wallet_to_link.key().as_ref()],
        bump
    )]
    pub wallet_link: Account<'info, VaultWalletLink>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UnlinkWallet<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
        has_one = authority @ TorchMarketError::VaultUnauthorized,
    )]
    pub vault: Account<'info, TorchVault>,
    /// CHECK: The wallet being unlinked
    pub wallet_to_unlink: UncheckedAccount<'info>,
    #[account(
        mut,
        close = authority,
        seeds = [VAULT_WALLET_LINK_SEED, wallet_to_unlink.key().as_ref()],
        bump = wallet_link.bump,
        constraint = wallet_link.vault == vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub wallet_link: Account<'info, VaultWalletLink>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TransferVaultAuthority<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
        has_one = authority @ TorchMarketError::VaultUnauthorized,
    )]
    pub vault: Account<'info, TorchVault>,
    /// CHECK: New authority wallet
    pub new_authority: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct WithdrawTokens<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, vault.creator.as_ref()],
        bump = vault.bump,
        has_one = authority @ TorchMarketError::VaultUnauthorized,
    )]
    pub vault: Account<'info, TorchVault>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(mut)]
    pub destination_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct VaultSwap<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Account<'info, TorchVault>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, signer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key()
            @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Account<'info, VaultWalletLink>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.migrated @ TorchMarketError::NotMigrated,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault,
        associated_token::token_program = token_2022_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct EnableShortSelling<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        constraint = global_config.authority == authority.key() @ TorchMarketError::Unauthorized,
    )]
    pub global_config: Box<Account<'info, GlobalConfig>>,
    /// CHECK: Mint account for PDA derivation
    pub mint: AccountInfo<'info>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.migrated @ TorchMarketError::NotMigrated,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        constraint = treasury.lending_enabled @ TorchMarketError::LendingNotEnabled,
        constraint = treasury.buyback_percent_bps != SHORT_ENABLED_SENTINEL @ TorchMarketError::ShortAlreadyEnabled,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        init,
        payer = authority,
        space = ShortConfig::LEN,
        seeds = [SHORT_CONFIG_SEED, mint.key().as_ref()],
        bump,
    )]
    pub short_config: Box<Account<'info, ShortConfig>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct OpenShort<'info> {
    #[account(mut)]
    pub shorter: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.migrated @ TorchMarketError::NotMigrated,
        constraint = !bonding_curve.reclaimed @ TorchMarketError::AlreadyReclaimed,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        constraint = treasury.buyback_percent_bps == SHORT_ENABLED_SENTINEL @ TorchMarketError::ShortNotEnabled,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init_if_needed,
        payer = shorter,
        space = ShortConfig::LEN,
        seeds = [SHORT_CONFIG_SEED, mint.key().as_ref()],
        bump,
    )]
    pub short_config: Box<Account<'info, ShortConfig>>,
    #[account(
        init_if_needed,
        payer = shorter,
        space = ShortPosition::LEN,
        seeds = [SHORT_SEED, mint.key().as_ref(), shorter.key().as_ref()],
        bump,
    )]
    pub short_position: Box<Account<'info, ShortPosition>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = shorter,
        associated_token::token_program = token_program,
    )]
    pub shorter_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(mut)]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseShort<'info> {
    #[account(mut)]
    pub shorter: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [SHORT_CONFIG_SEED, mint.key().as_ref()],
        bump = short_config.bump,
    )]
    pub short_config: Box<Account<'info, ShortConfig>>,
    #[account(
        mut,
        seeds = [SHORT_SEED, mint.key().as_ref(), shorter.key().as_ref()],
        bump = short_position.bump,
        constraint = short_position.tokens_borrowed > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub short_position: Box<Account<'info, ShortPosition>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = shorter,
        associated_token::token_program = token_program,
    )]
    pub shorter_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(mut)]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LiquidateShort<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    /// CHECK: Borrower wallet, receives rent on position close
    #[account(mut)]
    pub borrower: AccountInfo<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [SHORT_CONFIG_SEED, mint.key().as_ref()],
        bump = short_config.bump,
    )]
    pub short_config: Box<Account<'info, ShortConfig>>,
    #[account(
        mut,
        seeds = [SHORT_SEED, mint.key().as_ref(), borrower.key().as_ref()],
        bump = short_position.bump,
        constraint = short_position.tokens_borrowed > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub short_position: Box<Account<'info, ShortPosition>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    #[account(mut)]
    pub torch_vault: Option<Box<Account<'info, TorchVault>>>,
    pub vault_wallet_link: Option<Box<Account<'info, VaultWalletLink>>>,
    #[account(mut)]
    pub vault_token_account: Option<Box<InterfaceAccount<'info, TokenAccountInterface>>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
