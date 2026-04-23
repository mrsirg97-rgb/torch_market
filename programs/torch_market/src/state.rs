use anchor_lang::prelude::*;

#[account]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub dev_wallet: Pubkey,
    pub protocol_fee_bps: u16,
    pub paused: bool,
    pub total_tokens_launched: u64,
    pub total_volume_sol: u64,
    pub bump: u8,
}

impl GlobalConfig {
    pub const LEN: usize = 8  // discriminator
        + 32  // authority
        + 32  // treasury
        + 32  // dev_wallet
        + 2   // protocol_fee_bps
        + 1   // paused
        + 8   // total_tokens_launched
        + 8   // total_volume_sol
        + 1; // bump
}

#[account]
pub struct BondingCurve {
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub bonding_complete: bool,
    pub bonding_complete_slot: u64,
    pub migrated: bool,
    pub last_activity_slot: u64,
    pub reclaimed: bool,
    pub bump: u8,
    pub treasury_bump: u8,
    pub bonding_target: u64,
}

impl BondingCurve {
    pub const LEN: usize = 8   // discriminator
        + 32  // mint
        + 32  // creator
        + 8   // virtual_sol_reserves
        + 8   // virtual_token_reserves
        + 8   // real_sol_reserves
        + 8   // real_token_reserves
        + 1   // bonding_complete
        + 8   // bonding_complete_slot
        + 1   // migrated
        + 8   // last_activity_slot
        + 1   // reclaimed
        + 1   // bump
        + 1   // treasury_bump
        + 8; // bonding_target
}

#[account]
pub struct UserPosition {
    pub user: Pubkey,
    pub bonding_curve: Pubkey,
    pub total_purchased: u64,
    pub tokens_received: u64,
    pub tokens_burned: u64,
    pub total_sol_spent: u64,
    pub bump: u8,
}

impl UserPosition {
    pub const LEN: usize = 8  // discriminator
        + 32  // user
        + 32  // bonding_curve
        + 8   // total_purchased
        + 8   // tokens_received
        + 8   // tokens_burned
        + 8   // total_sol_spent
        + 1; // bump
}

#[account]
pub struct Treasury {
    pub bonding_curve: Pubkey,
    pub mint: Pubkey,
    pub sol_balance: u64,
    // Flag: this token was created as a community token (0% creator fees,
    // 100% of post-fee SOL to treasury). Replaces the `total_bought_back`
    // u64::MAX sentinel from pre-v20.
    pub is_community_token: bool,
    // SOL reserved as collateral by active short positions. Subtracted from
    // sol_balance when computing available-to-lend. Replaces the repurposed
    // `total_burned_from_buyback` counter from pre-v20.
    pub short_collateral_reserved: u64,
    pub last_buyback_slot: u64,
    pub harvested_fees: u64,
    pub bump: u8,
    // Baseline for post-migration ratio gating on `swap_fees_to_sol`.
    pub baseline_sol_reserves: u64,
    pub baseline_token_reserves: u64,
    // Flag: short selling has been enabled for this token. Replaces the
    // `buyback_percent_bps == u16::MAX` sentinel from pre-v20.
    pub short_selling_enabled: bool,
    pub min_buyback_interval_slots: u64,
    pub baseline_initialized: bool,
    pub total_stars: u64,
    pub star_sol_balance: u64,
    pub creator_paid_out: bool,
    // Treasury lending state.
    pub total_sol_lent: u64,
    pub total_collateral_locked: u64,
    pub active_loans: u64,
    pub total_interest_collected: u64,
    pub lending_enabled: bool,
    pub interest_rate_bps: u16,
    pub max_ltv_bps: u16,
    pub liquidation_threshold_bps: u16,
    pub liquidation_bonus_bps: u16,
    pub liquidation_close_bps: u16,
    pub lending_utilization_cap_bps: u16,
}

impl Treasury {
    pub const LEN: usize = 8   // discriminator
        + 32  // bonding_curve
        + 32  // mint
        + 8   // sol_balance
        + 1   // is_community_token
        + 8   // short_collateral_reserved
        + 8   // last_buyback_slot
        + 8   // harvested_fees
        + 1   // bump
        + 8   // baseline_sol_reserves
        + 8   // baseline_token_reserves
        + 1   // short_selling_enabled
        + 8   // min_buyback_interval_slots
        + 1   // baseline_initialized
        + 8   // total_stars
        + 8   // star_sol_balance
        + 1   // creator_paid_out
        + 8   // total_sol_lent
        + 8   // total_collateral_locked
        + 8   // active_loans
        + 8   // total_interest_collected
        + 1   // lending_enabled
        + 2   // interest_rate_bps
        + 2   // max_ltv_bps
        + 2   // liquidation_threshold_bps
        + 2   // liquidation_bonus_bps
        + 2   // liquidation_close_bps
        + 2; // lending_utilization_cap_bps
}

#[account]
pub struct UserStats {
    pub user: Pubkey,
    pub total_volume: u64,
    pub volume_current_epoch: u64,
    pub volume_previous_epoch: u64,
    pub last_epoch_claimed: u64,
    pub total_rewards_claimed: u64,
    pub last_volume_epoch: u64,
    pub bump: u8,
}

impl UserStats {
    pub const LEN: usize = 8   // discriminator
        + 32  // user
        + 8   // total_volume
        + 8   // volume_current_epoch
        + 8   // volume_previous_epoch
        + 8   // last_epoch_claimed
        + 8   // total_rewards_claimed
        + 8   // last_volume_epoch
        + 1; // bump
}

#[account]
pub struct StarRecord {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub starred_at_slot: u64,
    pub bump: u8,
}

impl StarRecord {
    pub const LEN: usize = 8   // discriminator
        + 32  // user
        + 32  // mint
        + 8   // starred_at_slot
        + 1; // bump
}

#[account]
pub struct ProtocolTreasury {
    pub authority: Pubkey,
    pub current_balance: u64,
    pub reserve_floor: u64,
    pub total_fees_received: u64,
    pub total_distributed: u64,
    pub current_epoch: u64,
    pub last_epoch_ts: i64,
    pub total_volume_current_epoch: u64,
    pub total_volume_previous_epoch: u64,
    pub distributable_amount: u64,
    pub bump: u8,
}

impl ProtocolTreasury {
    pub const LEN: usize = 8   // discriminator
        + 32  // authority
        + 8   // current_balance
        + 8   // reserve_floor
        + 8   // total_fees_received
        + 8   // total_distributed
        + 8   // current_epoch
        + 8   // last_epoch_ts
        + 8   // total_volume_current_epoch
        + 8   // total_volume_previous_epoch
        + 8   // distributable_amount
        + 1; // bump
}

#[account]
pub struct LoanPosition {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub collateral_amount: u64,
    pub borrowed_amount: u64,
    pub accrued_interest: u64,
    pub last_update_slot: u64,
    pub bump: u8,
}

impl LoanPosition {
    pub const LEN: usize = 8   // discriminator
        + 32  // user
        + 32  // mint
        + 8   // collateral_amount
        + 8   // borrowed_amount
        + 8   // accrued_interest
        + 8   // last_update_slot
        + 1; // bump
}

#[account]
pub struct TorchVault {
    pub creator: Pubkey,
    pub authority: Pubkey,
    pub sol_balance: u64,
    pub total_deposited: u64,
    pub total_withdrawn: u64,
    pub total_spent: u64,
    pub total_received: u64,
    pub linked_wallets: u8,
    pub created_at: i64,
    pub bump: u8,
}

impl TorchVault {
    pub const LEN: usize = 8   // discriminator
        + 32  // creator
        + 32  // authority
        + 8   // sol_balance
        + 8   // total_deposited
        + 8   // total_withdrawn
        + 8   // total_spent
        + 8   // total_received
        + 1   // linked_wallets
        + 8   // created_at
        + 1; // bump
}

#[account]
pub struct VaultWalletLink {
    pub vault: Pubkey,
    pub wallet: Pubkey,
    pub linked_at: i64,
    pub bump: u8,
}

impl VaultWalletLink {
    pub const LEN: usize = 8   // discriminator
        + 32  // vault
        + 32  // wallet
        + 8   // linked_at
        + 1; // bump
}

#[account]
pub struct TreasuryLock {
    pub mint: Pubkey,
    pub bump: u8,
}

impl TreasuryLock {
    pub const LEN: usize = 8   // discriminator
        + 32  // mint
        + 1; // bump
}

#[account]
pub struct ShortPosition {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub sol_collateral: u64,
    pub tokens_borrowed: u64,
    pub accrued_interest: u64,
    pub last_update_slot: u64,
    pub bump: u8,
}

impl ShortPosition {
    pub const LEN: usize = 8   // discriminator
        + 32  // user
        + 32  // mint
        + 8   // sol_collateral
        + 8   // tokens_borrowed
        + 8   // accrued_interest
        + 8   // last_update_slot
        + 1; // bump
}

#[account]
pub struct ShortConfig {
    pub mint: Pubkey,
    pub total_tokens_lent: u64,
    pub active_positions: u64,
    pub total_interest_collected: u64,
    pub bump: u8,
}

impl ShortConfig {
    pub const LEN: usize = 8   // discriminator
        + 32  // mint
        + 8   // total_tokens_lent
        + 8   // active_positions
        + 8   // total_interest_collected
        + 1; // bump
}
