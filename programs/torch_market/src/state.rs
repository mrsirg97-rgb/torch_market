use anchor_lang::prelude::*;

#[account]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub dev_wallet: Pubkey,
    pub _deprecated_platform_treasury: Pubkey, // Layout compat — replaced by ProtocolTreasury PDA
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
        + 32  // dev_wallet [V8]
        + 32  // _deprecated_platform_treasury [V4, deprecated V3.2]
        + 2   // protocol_fee_bps
        + 1   // paused
        + 8   // total_tokens_launched
        + 8   // total_volume_sol
        + 1;  // bump
}

#[account]
pub struct BondingCurve {
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    // V36: vote fields below are deprecated — kept for Borsh layout compatibility.
    // New tokens initialize vote_finalized=true so migration gate passes without votes.
    pub vote_vault_balance: u64,
    pub permanently_burned_tokens: u64,
    pub bonding_complete: bool,
    pub bonding_complete_slot: u64,
    pub votes_return: u64,
    pub votes_burn: u64,
    pub total_voters: u64,
    pub vote_finalized: bool,
    pub vote_result_return: bool,
    pub migrated: bool,
    pub is_token_2022: bool,
    pub last_activity_slot: u64,
    pub reclaimed: bool,
    pub name: [u8; 32],
    pub symbol: [u8; 10],
    pub uri: [u8; 200],
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
        + 8   // vote_vault_balance [V13: renamed from burned_token_reserves]
        + 8   // permanently_burned_tokens [V2]
        + 1   // bonding_complete
        + 8   // bonding_complete_slot
        + 8   // votes_return
        + 8   // votes_burn
        + 8   // total_voters
        + 1   // vote_finalized
        + 1   // vote_result_return
        + 1   // migrated
        + 1   // is_token_2022 (V3)
        + 8   // last_activity_slot [V4]
        + 1   // reclaimed [V4]
        + 32  // name
        + 10  // symbol
        + 200 // uri
        + 1   // bump
        + 1   // treasury_bump [V2]
        + 8;  // bonding_target [V23]
}

#[account]
pub struct UserPosition {
    pub user: Pubkey,
    pub bonding_curve: Pubkey,
    pub total_purchased: u64,
    pub tokens_received: u64,
    pub tokens_burned: u64,
    pub total_sol_spent: u64,
    pub has_voted: bool,
    pub vote_return: bool,
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
        + 1   // has_voted
        + 1   // vote_return
        + 1;  // bump
}

#[account]
pub struct Treasury {
    pub bonding_curve: Pubkey,
    pub mint: Pubkey,
    pub sol_balance: u64,
    // Sentinel u64::MAX = community token (no creator fees). Otherwise legacy counter.
    pub total_bought_back: u64,
    // Repurposed: tracks total SOL collateral locked by short sellers when shorts enabled.
    pub total_burned_from_buyback: u64,
    pub tokens_held: u64,
    pub last_buyback_slot: u64,
    pub buyback_count: u64,
    pub harvested_fees: u64,
    pub bump: u8,

    pub baseline_sol_reserves: u64,
    pub baseline_token_reserves: u64,
    pub ratio_threshold_bps: u16,
    pub reserve_ratio_bps: u16,
    // Sentinel u16::MAX = short selling enabled for this token.
    pub buyback_percent_bps: u16,
    pub min_buyback_interval_slots: u64,
    pub baseline_initialized: bool,

    pub total_stars: u64,
    pub star_sol_balance: u64,
    pub creator_paid_out: bool,

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
        + 8   // total_bought_back
        + 8   // total_burned_from_buyback
        + 8   // tokens_held
        + 8   // last_buyback_slot
        + 8   // buyback_count
        + 8   // harvested_fees (V3)
        + 1   // bump
        // V9: Auto Buyback Config
        + 8   // baseline_sol_reserves
        + 8   // baseline_token_reserves
        + 2   // ratio_threshold_bps
        + 2   // reserve_ratio_bps
        + 2   // buyback_percent_bps
        + 8   // min_buyback_interval_slots
        + 1   // baseline_initialized
        // V10: Star/Creator Payout
        + 8   // total_stars
        + 8   // star_sol_balance
        + 1   // creator_paid_out
        // V2.4: Treasury Lending
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
        + 2;  // lending_utilization_cap_bps
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
        + 1;  // bump
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
        + 1;  // bump
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
        + 1;  // bump
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
        + 1;  // bump
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
        + 8   // total_received [V18]
        + 1   // linked_wallets
        + 8   // created_at
        + 1;  // bump
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
        + 1;  // bump
}

#[account]
pub struct TreasuryLock {
    pub mint: Pubkey,
    pub bump: u8,
}

impl TreasuryLock {
    pub const LEN: usize = 8   // discriminator
        + 32  // mint
        + 1;  // bump
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
        + 1;  // bump
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
        + 1;  // bump
}
