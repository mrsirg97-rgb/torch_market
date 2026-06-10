use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{
        Mint as MintInterface, TokenAccount as TokenAccountInterface, TokenInterface,
    },
};

use crate::constants::*;
use crate::errors::TorchMarketError;
use crate::pool_validation::{
    derive_deep_pool, derive_deep_pool_event_authority, derive_deep_pool_lp_mint,
    derive_deep_pool_vault, derive_torch_config,
};
use crate::state::*;
use crate::token_2022_utils::{get_associated_token_address_2022, TOKEN_2022_PROGRAM_ID};

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

// [V21] Per-token closed leverage args. `position_index` selects which position
// in a (user, mint, side) set is opened/acted-on (supports DCA / scaled entries).
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct OpenPositionArgs {
    pub position_index: u32,
    /// Collateral deposited: SOL lamports (short) or tokens (long).
    pub collateral: u64,
    /// Slippage guard on the atomic open swap: min SOL out from the sale
    /// (short) or min tokens out from the buy (long).
    pub min_out: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ClosePositionArgs {
    pub position_index: u32,
    /// 10000 = full close; smaller = partial (scales the swap + debt repaid).
    pub repay_fraction_bps: u16,
    /// Slippage guard: minimum surplus SOL returned to the user.
    pub min_surplus_sol_out: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct LiquidatePositionArgs {
    pub position_index: u32,
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

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: CreateTokenArgs)]
pub struct CreateToken2022<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
    )]
    pub global_config: Box<Account<'info, GlobalConfig>>,
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
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — materialized (rent-funded) in
    // the handler so all later SOL flows are plain system_program::transfers.
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
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

// Wallet-funded buy on the bonding curve. For vault-routed buys, use `BuyViaVault`.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: BuyArgs)]
pub struct Buy<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        constraint = args.sol_amount >= MIN_SOL_AMOUNT @ TorchMarketError::AmountTooSmall,
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
    /// CHECK: System-owned bonding_curve_sol PDA (SOL custody for the curve).
    /// Address validated in the handler — bare AccountInfo to spare try_accounts
    /// stack on this heavy event_cpi context.
    #[account(mut)]
    pub bonding_curve_sol: AccountInfo<'info>,
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
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// Vault-routed buy. Vault accounts are MANDATORY (not Optional) — no `unwrap()`
// in constraints. The signer is a linked controller wallet acting on behalf of
// the vault; vault holds the SOL paid and receives the tokens.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: BuyArgs)]
pub struct BuyViaVault<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
        constraint = args.sol_amount >= MIN_SOL_AMOUNT @ TorchMarketError::AmountTooSmall,
    )]
    pub global_config: Box<Account<'info, GlobalConfig>>,
    /// CHECK: Dev wallet
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
    /// CHECK: System-owned bonding_curve_sol PDA (SOL custody for the curve).
    /// Address validated in the handler — bare AccountInfo to spare try_accounts
    /// stack on this heavy event_cpi context.
    #[account(mut)]
    pub bonding_curve_sol: AccountInfo<'info>,
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
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, buyer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault,
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// Wallet-funded sell on the bonding curve. For vault-routed sells, use `SellViaVault`.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: SellArgs)]
pub struct Sell<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = !bonding_curve.bonding_complete @ TorchMarketError::BondingComplete,
        constraint = !bonding_curve.reclaimed @ TorchMarketError::AlreadyReclaimed,
        constraint = args.token_amount > 0 @ TorchMarketError::ZeroAmount,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    /// CHECK: System-owned bonding_curve_sol PDA (SOL custody for the curve).
    /// Address validated in the handler — bare AccountInfo to spare try_accounts
    /// stack on this heavy event_cpi context.
    #[account(mut)]
    pub bonding_curve_sol: AccountInfo<'info>,
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
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

// Vault-routed sell. Vault accounts MANDATORY. Tokens come from vault ATA, SOL
// proceeds go to vault.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: SellArgs)]
pub struct SellViaVault<'info> {
    #[account(mut)]
    pub seller: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = !bonding_curve.bonding_complete @ TorchMarketError::BondingComplete,
        constraint = !bonding_curve.reclaimed @ TorchMarketError::AlreadyReclaimed,
        constraint = args.token_amount > 0 @ TorchMarketError::ZeroAmount,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    /// CHECK: System-owned bonding_curve_sol PDA (SOL custody for the curve).
    /// Address validated in the handler — bare AccountInfo to spare try_accounts
    /// stack on this heavy event_cpi context.
    #[account(mut)]
    pub bonding_curve_sol: AccountInfo<'info>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = bonding_curve,
        associated_token::token_program = token_program,
    )]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
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
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, seller.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault,
        associated_token::token_program = token_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
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
        constraint = treasury.baseline_initialized @ TorchMarketError::BaselineNotInitialized,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
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
    /// CHECK: DeepPool event_authority PDA — required by deep_pool's #[event_cpi]
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Token-2022 program for project tokens
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
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
    /// CHECK: System-owned bonding_curve_sol PDA — seeds-validated SOL custody.
    #[account(
        mut,
        seeds = [BONDING_CURVE_SOL_SEED, mint.key().as_ref()],
        bump,
    )]
    pub bonding_curve_sol: AccountInfo<'info>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = bonding_curve.treasury_bump,
    )]
    pub token_treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [PROTOCOL_TREASURY_SEED],
        bump = protocol_treasury.bump,
    )]
    pub protocol_treasury: Box<Account<'info, ProtocolTreasury>>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
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
    /// CHECK: System-owned bonding_curve_sol PDA — seeds-validated SOL custody.
    #[account(
        mut,
        seeds = [BONDING_CURVE_SOL_SEED, mint.key().as_ref()],
        bump,
    )]
    pub bonding_curve_sol: AccountInfo<'info>,
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

// Wallet-funded protocol reward claim. For vault-routed, use `ClaimProtocolRewardsViaVault`.
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
}

// Vault-routed protocol reward claim. Claimed SOL goes to vault instead of user.
#[derive(Accounts)]
pub struct ClaimProtocolRewardsViaVault<'info> {
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
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, user.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
pub struct MigrateToDex<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [BONDING_CURVE_SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.bonding_complete @ TorchMarketError::BondingNotComplete,
        constraint = !bonding_curve.migrated @ TorchMarketError::AlreadyMigrated,
    )]
    pub bonding_curve: Box<Account<'info, BondingCurve>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — reimbursement source on migrate.
    // Fully validated (owner + seeds); room freed by dropping the unused global_config.
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    /// CHECK: System-owned bonding_curve_sol PDA holding the bonded SOL. Address +
    /// bump validated manually in the handler to reduce try_accounts stack pressure
    /// (this context is near the 4KB SBF frame limit). Seed-signed in the handler as
    /// deep_pool create_pool's sol_source, so the raise flows curve → pool without
    /// ever touching a user wallet.
    #[account(mut)]
    pub bonding_curve_sol: AccountInfo<'info>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = bonding_curve,
        associated_token::token_program = token_2022_program,
    )]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
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
    /// CHECK: Payer's LP ATA — receives LP tokens from create_pool, then burned.
    /// Address-constrained to the canonical Token-2022 ATA(payer, deep_pool_lp_mint)
    /// so a malformed account fails at constraint time instead of inside the CPI.
    #[account(
        mut,
        address = get_associated_token_address_2022(&payer.key(), &deep_pool_lp_mint.key()) @ TorchMarketError::InvalidPoolAccount,
    )]
    pub payer_lp_account: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA's LP ATA — receives locked LP from create_pool.
    /// Address-constrained to the canonical Token-2022 ATA(deep_pool, deep_pool_lp_mint).
    #[account(
        mut,
        address = get_associated_token_address_2022(&deep_pool.key(), &deep_pool_lp_mint.key()) @ TorchMarketError::InvalidPoolAccount,
    )]
    pub deep_pool_lp_account: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA — required by deep_pool's #[event_cpi]
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Token-2022 program for project tokens
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
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
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds;
    /// materialized (rent-funded) in the handler. All vault SOL lives here.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, creator.key().as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
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
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
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
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
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
    // Authority-chosen destination; constrain the mint so a wrong-mint account fails
    // at constraint time rather than deep inside the transfer_checked CPI.
    #[account(mut, token::mint = mint)]
    pub destination_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[event_cpi]
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
    /// CHECK: System-owned PDA used as sol_source in the deep_pool swap CPI.
    /// 0 bytes, holds SOL only for the duration of a swap.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
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
    /// CHECK: DeepPool event_authority PDA — required by deep_pool's #[event_cpi]
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// ============================================================================
// [V21] Per-token closed leveraged markets — contexts (direct variants).
//
// Atomic open/close route through deep_pool::swap in a single CPI:
//   open_short : SELL — user=treasury_lock (lock ATA = token source),
//                sol_source=position_sol_vault (SOL sink). Borrow+sell atomic.
//   close_short: BUY  — user=treasury_lock (lock ATA = repay sink),
//                sol_source=position_sol_vault (SOL source).
//   open_long  : BUY  — user=position (its ATA = token sink),
//                sol_source=long_sol_vault (treasury SOL staged here).
//   close_long : SELL — user=position (its ATA = token source),
//                sol_source=long_sol_vault (SOL sink, then split).
// Liquidations transfer directly (no swap): liquidator pays the debt asset,
// seizes the vault asset + bonus.
//
// Per-position vaults (D-2): position_sol_vault is a system-owned 0-data PDA
// (lamports only); position_token_vault is the canonical ATA of the Position
// PDA (deep_pool's Swap requires user_token_account == ATA(user)). The Position
// PDA seed carries a side byte so a long + short at the same index don't collide.
// ============================================================================

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: OpenPositionArgs)]
pub struct OpenShortPosition<'info> {
    #[account(mut)]
    pub shorter: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
        constraint = treasury.short_selling_enabled @ TorchMarketError::ShortNotEnabled,
        constraint = args.collateral > 0 @ TorchMarketError::EmptyBorrowRequest,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — ALL treasury SOL lives here
    // (unified; the spendable balance is derived from its lamports, never a field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    // mut: passed as deep_pool Swap's `user`, which deep_pool marks writable.
    #[account(
        mut,
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    // Token source for the atomic sell (deep_pool pulls from this ATA, whose
    // authority is treasury_lock).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // [V21] short aggregate counters moved to Treasury (total_tokens_lent /
    // active_shorts / short_interest_collected); ShortConfig deleted.
    #[account(
        init,
        payer = shorter,
        space = Position::LEN,
        seeds = [POSITION_SEED, shorter.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — enforces the per-USER
    // caps across position_index values. Lazily created on first open.
    // Zero-copy loader: a borsh Account here overflows the 4096-byte
    // try_accounts stack frame on the heavier via_vault contexts.
    #[account(
        init_if_needed,
        payer = shorter,
        space = UserRisk::LEN,
        seeds = [USER_RISK_SEED, shorter.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: system-owned per-position SOL vault (0 data, holds lamports only).
    /// Receives net collateral + atomic-sale proceeds; sol_source of the sell.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, shorter.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA — required by deep_pool's #[event_cpi]
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed open_short. Collateral comes from the vault's System-owned
// vault_sol (seed-signed), the position is VAULT-SEEDED (owned by the vault, any
// linked wallet manages it), and `signer` is any linked wallet (proven by
// vault_wallet_link) that pays the position rent. See docs/v21-closed-loop-leverage.md D-12.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: OpenPositionArgs)]
pub struct OpenShortViaVault<'info> {
    // A linked wallet — executes the trade, pays the position rent. NOT the funds source.
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds;
    /// the collateral source (seed-signed). All vault SOL lives here.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    // Auth: `signer` must be a linked wallet of THIS vault (can trade, can't extract).
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, signer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
        constraint = treasury.short_selling_enabled @ TorchMarketError::ShortNotEnabled,
        constraint = args.collateral > 0 @ TorchMarketError::EmptyBorrowRequest,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    // mut: passed as deep_pool Swap's `user`, which deep_pool marks writable.
    #[account(
        mut,
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // VAULT-SEEDED position — owned by the vault, manageable by any linked wallet.
    #[account(
        init,
        payer = signer,
        space = Position::LEN,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — enforces the per-USER
    // caps across position_index values. Lazily created on first open.
    // Zero-copy loader: a borsh Account here overflows the 4096-byte
    // try_accounts stack frame on the heavier via_vault contexts.
    #[account(
        init_if_needed,
        payer = signer,
        space = UserRisk::LEN,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: vault-seeded per-position SOL vault (0 data). Receives net collateral
    /// + atomic-sale proceeds; sol_source of the sell.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA — required by deep_pool's #[event_cpi]
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ClosePositionArgs)]
pub struct CloseShortPosition<'info> {
    #[account(mut)]
    pub shorter: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // mut: passed as deep_pool Swap's `user`, which deep_pool marks writable.
    #[account(
        mut,
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    // Repay sink for the atomic buy: bought tokens land back in the lock ATA.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [POSITION_SEED, shorter.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, shorter.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: system-owned per-position SOL vault. SOL source for the buy +
    /// holds the leftover surplus returned to the user.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, shorter.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump = position.vault_bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed close_short. Vault-seeded position; surplus SOL → vault_sol;
// position rent → signer (the linked wallet's own gas, never vault funds). D-12.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ClosePositionArgs)]
pub struct CloseShortViaVault<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — surplus sink (seed-validated).
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, signer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // mut: passed as deep_pool Swap's `user`, which deep_pool marks writable.
    #[account(
        mut,
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: vault-seeded per-position SOL vault. SOL source for the buy + holds
    /// the leftover surplus, which returns to the vault on full close.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump = position.vault_bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: LiquidatePositionArgs)]
pub struct LiquidateShortPosition<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    /// CHECK: borrower wallet — receives rent + any residual vault SOL on full close
    #[account(mut)]
    pub borrower: AccountInfo<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    // Liquidator's debt-cover tokens land here (repays the borrow).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        mut,
        seeds = [POSITION_SEED, borrower.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, borrower.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: system-owned per-position SOL vault — seized SOL paid to liquidator.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, borrower.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump = position.vault_bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    // Source of the liquidator's debt-cover tokens.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_2022_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA — read for LTV pricing (no swap CPI here)
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault — read for LTV pricing
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed liquidation of a vault-owned short. The liquidator is an
// EXTERNAL actor (no vault_wallet_link — anyone can liquidate); seized SOL is
// paid to the liquidator, while residual SOL and the position rent on full
// liquidation flow back to the vault's System-owned vault_sol (value never
// leaves the vault). See docs/v21-closed-loop-leverage.md D-12.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: LiquidatePositionArgs)]
pub struct LiquidateShortViaVault<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    /// Receives residual SOL + position rent on full liquidation (seed-signed
    /// for the residual transfer; direct close-account credit for the rent).
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        seeds = [TREASURY_LOCK_SEED, mint.key().as_ref()],
        bump = treasury_lock.bump,
    )]
    pub treasury_lock: Box<Account<'info, TreasuryLock>>,
    // Liquidator's debt-cover tokens land here (repays the borrow).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = treasury_lock,
        associated_token::token_program = token_2022_program,
    )]
    pub treasury_lock_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // VAULT-SEEDED position — owned by the vault.
    #[account(
        mut,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_SHORT], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveShort,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    /// CHECK: vault-seeded per-position SOL vault — seized SOL paid to liquidator,
    /// residual flows to vault_sol.
    #[account(
        mut,
        seeds = [SHORT_VAULT_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump = position.vault_bump,
    )]
    pub position_sol_vault: AccountInfo<'info>,
    // Source of the liquidator's debt-cover tokens.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_2022_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA — read for LTV pricing (no swap CPI here)
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault — read for LTV pricing
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: OpenPositionArgs)]
pub struct OpenLongPosition<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::LendingRequiresMigration,
        constraint = treasury.lending_enabled @ TorchMarketError::LendingNotEnabled,
        constraint = args.collateral > 0 @ TorchMarketError::EmptyBorrowRequest,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody for the treasury — the lend is sourced from here.
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    // Source of the user's token collateral.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = borrower,
        associated_token::token_program = token_2022_program,
    )]
    pub borrower_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    #[account(
        init,
        payer = borrower,
        space = Position::LEN,
        seeds = [POSITION_SEED, borrower.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — enforces the per-USER
    // caps across position_index values. Lazily created on first open.
    // Zero-copy loader: a borsh Account here overflows the 4096-byte
    // try_accounts stack frame on the heavier via_vault contexts.
    #[account(
        init_if_needed,
        payer = borrower,
        space = UserRisk::LEN,
        seeds = [USER_RISK_SEED, borrower.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    // Per-position token vault: canonical ATA of the Position PDA. Holds
    // collateral + atomically-bought tokens. deep_pool's Swap requires the
    // token account be the ATA of `user` (= position), so an ATA is mandatory.
    #[account(
        init,
        payer = borrower,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: system-owned transient SOL stage — treasury borrow lands here,
    /// then funds the atomic buy as deep_pool's sol_source. ~0 after the swap.
    #[account(
        mut,
        seeds = [LONG_SOL_VAULT_SEED, borrower.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub long_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ClosePositionArgs)]
pub struct CloseLongPosition<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    // mut: full-close harvests the position vault's withheld fees into the mint.
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody — debt repays land here. The spendable treasury
    // balance is DERIVED from these lamports (there is no tracked sol_balance field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [POSITION_SEED, borrower.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, borrower.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    // Token source for the atomic sell (ATA of the Position PDA).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: system-owned transient SOL sink — sell proceeds land here, then
    /// split into treasury (debt) + borrower (surplus).
    #[account(
        mut,
        seeds = [LONG_SOL_VAULT_SEED, borrower.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub long_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(args: LiquidatePositionArgs)]
pub struct LiquidateLongPosition<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    /// CHECK: borrower wallet — receives rent + any residual vault tokens on full close
    #[account(mut)]
    pub borrower: AccountInfo<'info>,
    // mut: full liquidation harvests the position vault's withheld fees into the mint.
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    // System-owned SOL custody — debt repays land here. The spendable treasury
    // balance is DERIVED from these lamports (there is no tracked sol_balance field).
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [POSITION_SEED, borrower.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, borrower.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    // Seized tokens come out of the position vault (ATA of the Position PDA).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // Sink for the borrower's residual equity tokens on full liquidation (the
    // partial seize leaves equity; unlike close_long which sells 100%). Mirror
    // of liquidate_short returning residual SOL — here the residual is tokens,
    // so it needs the borrower's ATA.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = borrower,
        associated_token::token_program = token_2022_program,
    )]
    pub borrower_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // Liquidator receives the seized tokens here — created on demand so a
    // liquidator never needs a pre-existing ATA (they pay its rent).
    #[account(
        init_if_needed,
        payer = liquidator,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_2022_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA — read for LTV pricing (no swap CPI here)
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault — read for LTV pricing
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed open_long. The token COLLATERAL comes from the vault's token
// ATA (vault_token_account, authority torch_vault — seed-signed), the borrowed
// SOL is lent from treasury_sol_vault (as in open_long), the position is
// VAULT-SEEDED (owned by the vault), and `signer` is any linked wallet (proven by
// vault_wallet_link) that pays the position + token-vault rent. The vault's SOL
// (vault_sol) is untouched — longs borrow from the treasury, not the vault.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: OpenPositionArgs)]
pub struct OpenLongViaVault<'info> {
    // A linked wallet — executes the trade, pays the position rent. NOT the funds source.
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    // Auth: `signer` must be a linked wallet of THIS vault (can trade, can't extract).
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, signer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::LendingRequiresMigration,
        constraint = treasury.lending_enabled @ TorchMarketError::LendingNotEnabled,
        constraint = args.collateral > 0 @ TorchMarketError::EmptyBorrowRequest,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    // Collateral SOURCE: the vault's token ATA (authority torch_vault, seed-signed).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault,
        associated_token::token_program = token_2022_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // VAULT-SEEDED position — owned by the vault, manageable by any linked wallet.
    #[account(
        init,
        payer = signer,
        space = Position::LEN,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — enforces the per-USER
    // caps across position_index values. Lazily created on first open.
    // Zero-copy loader: a borsh Account here overflows the 4096-byte
    // try_accounts stack frame on the heavier via_vault contexts.
    #[account(
        init_if_needed,
        payer = signer,
        space = UserRisk::LEN,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    #[account(
        init,
        payer = signer,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: vault-seeded transient SOL stage — treasury borrow lands here,
    /// then funds the atomic buy as deep_pool's sol_source. ~0 after the swap.
    #[account(
        mut,
        seeds = [LONG_SOL_VAULT_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub long_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed close_long. Sells the vault-owned position's tokens for SOL,
// repays the debt to treasury_sol_vault, and returns surplus P&L to the vault's
// vault_sol (NOT the linked wallet). `signer` (a linked wallet) gets only its own
// position + token-vault rent back on full close.
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: ClosePositionArgs)]
pub struct CloseLongViaVault<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    /// Surplus P&L returns here (seed-signed by the position vault).
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    #[account(
        seeds = [VAULT_WALLET_LINK_SEED, signer.key().as_ref()],
        bump = vault_wallet_link.bump,
        constraint = vault_wallet_link.vault == torch_vault.key() @ TorchMarketError::VaultWalletLinkMismatch,
    )]
    pub vault_wallet_link: Box<Account<'info, VaultWalletLink>>,
    // mut: full-close harvests the position vault's withheld fees into the mint.
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: vault-seeded transient SOL sink — sell proceeds land here, then
    /// split into treasury (debt) + vault_sol (surplus).
    #[account(
        mut,
        seeds = [LONG_SOL_VAULT_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &args.position_index.to_le_bytes()],
        bump,
    )]
    pub long_sol_vault: AccountInfo<'info>,
    /// CHECK: DeepPool program - validated by address constraint
    #[account(address = DEEP_POOL_PROGRAM_ID)]
    pub deep_pool_program: AccountInfo<'info>,
    /// CHECK: DeepPool pool PDA - validated by address constraint
    #[account(mut, address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault - validated by address constraint
    #[account(mut, address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: DeepPool event_authority PDA
    #[account(address = derive_deep_pool_event_authority() @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool_event_authority: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// [V21] Vault-routed liquidation of a vault-owned long. EXTERNAL liquidator (no
// vault_wallet_link) repays SOL debt → treasury, seizes vault tokens + bonus.
// On full liquidation, residual equity tokens → the vault's token ATA and the
// position + token-vault rent → the vault's vault_sol (value never leaves the vault).
#[event_cpi]
#[derive(Accounts)]
#[instruction(args: LiquidatePositionArgs)]
pub struct LiquidateLongViaVault<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(
        mut,
        seeds = [TORCH_VAULT_SEED, torch_vault.creator.as_ref()],
        bump = torch_vault.bump,
    )]
    pub torch_vault: Box<Account<'info, TorchVault>>,
    /// CHECK: System-owned SOL home for the vault — address-validated by seeds.
    /// Receives the position + token-vault rent on full liquidation.
    #[account(
        mut,
        seeds = [TORCH_VAULT_SOL_SEED, torch_vault.creator.as_ref()],
        bump,
    )]
    pub vault_sol: AccountInfo<'info>,
    // mut: full liquidation harvests the position vault's withheld fees into the mint.
    #[account(mut)]
    pub mint: Box<InterfaceAccount<'info, MintInterface>>,
    #[account(
        mut,
        seeds = [TREASURY_SEED, mint.key().as_ref()],
        bump = treasury.bump,
        // Migrated ⟺ baseline_initialized (set once in migrate_to_dex);
        // replaces the bonding_curve account that existed in this context
        // only for the migrated/!reclaimed checks (reclaim is mutually
        // exclusive with bonding completion). Dropping the account frees
        // try_accounts stack and one account per leverage tx.
        constraint = treasury.baseline_initialized @ TorchMarketError::NotMigrated,
    )]
    pub treasury: Box<Account<'info, Treasury>>,
    #[account(
        mut,
        seeds = [TREASURY_SOL_VAULT_SEED, mint.key().as_ref()],
        bump,
    )]
    pub treasury_sol_vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [POSITION_SEED, torch_vault.key().as_ref(), mint.key().as_ref(), &[POSITION_SIDE_LONG], &args.position_index.to_le_bytes()],
        bump = position.bump,
        constraint = position.debt_amount > 0 @ TorchMarketError::NoActiveLoan,
    )]
    pub position: Box<Account<'info, Position>>,
    // [F-1][F-3] Per-(owner, mint) aggregate exposure — debt repaid/written
    // off here releases the owner's per-user cap headroom. (Zero-copy loader —
    // see the open contexts.)
    #[account(
        mut,
        seeds = [USER_RISK_SEED, torch_vault.key().as_ref(), mint.key().as_ref()],
        bump,
    )]
    pub user_risk: AccountLoader<'info, UserRisk>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = position,
        associated_token::token_program = token_2022_program,
    )]
    pub position_token_vault: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // Sink for the vault's residual equity tokens on full liquidation (the vault's
    // own token ATA — value stays in the vault).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = torch_vault,
        associated_token::token_program = token_2022_program,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    // Liquidator receives the seized tokens here — created on demand.
    #[account(
        init_if_needed,
        payer = liquidator,
        associated_token::mint = mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_2022_program,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
    /// CHECK: DeepPool pool PDA — read for LTV pricing (no swap CPI here)
    #[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ TorchMarketError::InvalidPoolAccount)]
    pub deep_pool: AccountInfo<'info>,
    /// CHECK: DeepPool token vault — read for LTV pricing
    #[account(address = derive_deep_pool_vault(&deep_pool.key()) @ TorchMarketError::InvalidPoolVault)]
    pub deep_pool_token_vault: AccountInfo<'info>,
    /// CHECK: Validated by address constraint
    #[account(address = TOKEN_2022_PROGRAM_ID)]
    pub token_2022_program: AccountInfo<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}


// [V21][D-10] The permissionless TWAP crank (RecordObservation) was removed: the
// oracle moved into DeepPool, which is keeperless (it accumulates on every swap),
// so there is nothing for a crank to do. See docs/twap-oracle.md.
