use anchor_lang::prelude::*;
// Disambiguate `borsh` for the standalone AnchorSerialize/AnchorDeserialize
// derives (PositionSide, Observation). The litesvm dev-dependency pulls `borsh`
// as a direct extern, which collides with the bare `borsh::` path Anchor's derive
// macros emit when the lib is compiled in the test context. Pinning it to
// Anchor's own re-export resolves the ambiguity in both build + test.
use anchor_lang::prelude::borsh;

#[account]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub dev_wallet: Pubkey,
    pub protocol_fee_bps: u16,
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
    pub total_sol_spent: u64,
    pub bump: u8,
}

impl UserPosition {
    pub const LEN: usize = 8  // discriminator
        + 32  // user
        + 32  // bonding_curve
        + 8   // total_purchased
        + 8   // tokens_received
        + 8   // total_sol_spent
        + 1; // bump
}

// [V21][D-10] The TWAP oracle was lifted out of torch into DeepPool (keeperless;
// docs/twap-oracle.md). Torch no longer stores an observation ring — it reads a
// Q64.64 mark from the `deep_pool::Pool` at liquidation time. The `Observation`
// struct + Treasury ring that used to live here are gone.

#[account]
pub struct Treasury {
    pub bonding_curve: Pubkey,
    pub mint: Pubkey,
    // [V21] No `sol_balance` field — treasury SOL is DERIVED from the System-owned
    // treasury_sol_vault's lamports (single source of truth; can't drift). Tracked
    // the one obligation against it lives below: total_sol_lent_to_longs.
    // Flag: this token was created as a community token (0% creator fees,
    // 100% of post-fee SOL to treasury). Replaces the `total_bought_back`
    // u64::MAX sentinel from pre-v20.
    pub is_community_token: bool,
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
    // [V21] Treasury lending state (D-8). Collateral for both sides lives in
    // per-position vaults, NOT in the treasury — so the treasury only tracks
    // aggregate EXPOSURE (for the lending gate + utilization caps), never
    // custodies open-position assets.
    //
    // Shorts (token debt against per-position SOL vaults):
    pub total_tokens_lent: u64,        // aggregate gross token debt across open shorts
    pub active_shorts: u64,
    pub short_interest_collected: u64, // accrued short interest revenue (token-denom)
    // Longs (SOL debt against per-position token vaults):
    pub total_sol_lent_to_longs: u64,      // aggregate gross SOL debt across open longs
    pub active_longs: u64,
    pub long_interest_collected: u64,      // accrued long interest revenue (SOL-denom)
    pub total_token_collateral_locked: u64, // aggregate token collateral across open longs
    pub lending_enabled: bool,
    pub interest_rate_bps: u16,
    pub max_ltv_bps: u16,
    pub liquidation_threshold_bps: u16,
    pub liquidation_bonus_bps: u16,
    pub liquidation_close_bps: u16,
    pub lending_utilization_cap_bps: u16,
    // [V21][D-10] The TWAP mark now lives in DeepPool (read at liquidation time);
    // no observation ring is stored here. See docs/twap-oracle.md.
}

impl Treasury {
    pub const LEN: usize = 8   // discriminator
        + 32  // bonding_curve
        + 32  // mint
        + 1   // is_community_token
        + 8   // last_buyback_slot
        + 8   // harvested_fees
        + 1   // bump
        + 8   // baseline_sol_reserves
        + 8   // baseline_token_reserves
        + 1   // short_selling_enabled
        + 8   // min_buyback_interval_slots
        + 1   // baseline_initialized
        + 8   // total_tokens_lent
        + 8   // active_shorts
        + 8   // short_interest_collected
        + 8   // total_sol_lent_to_longs
        + 8   // active_longs
        + 8   // long_interest_collected
        + 8   // total_token_collateral_locked
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
pub struct TorchVault {
    pub creator: Pubkey,
    pub authority: Pubkey,
    // [V21] No `sol_balance` field — the vault's SOL is DERIVED from its
    // System-owned `vault_sol` PDA (vault_physical_sol). The totals below are
    // pure stats (deposit/spend history), never trusted for a balance.
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

// ============================================================================
// [V21] Per-token closed leverage
// ============================================================================

/// Side discriminant for a `Position`. `#[repr(u8)]` with explicit values so
/// the byte used in the Position PDA seed (constants::POSITION_SIDE_*) is
/// guaranteed to match the Borsh discriminant — a long and a short at the same
/// (user, mint, position_index) resolve to distinct PDAs via this byte.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PositionSide {
    Long = 0,
    Short = 1,
}

/// Unified leveraged position (D-3). Single shape, `side` discriminant. Both
/// sides share lifecycle, account layout, and math; only the held/borrowed
/// asset types mirror.
///
/// `held_amount` is intentionally NOT a field — the held asset IS the position
/// vault balance (`position_sol_vault.lamports()` for shorts,
/// `position_token_vault.amount` for longs). Read from the vault, not from
/// tracked state — derived state over tracked state makes the math unfalsifiable.
#[account]
pub struct Position {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub side: PositionSide,
    pub position_index: u32,
    pub collateral_amount: u64, // initial deposit, record-keeping only
    pub debt_amount: u64,       // owed asset: tokens for short, SOL (gross) for long
    pub accrued_interest: u64,  // in debt-asset denom
    pub last_slot: u64,
    pub bump: u8,
    pub vault_bump: u8, // bump of the position's SOL vault PDA
}

impl Position {
    pub const LEN: usize = 8   // discriminator
        + 32  // user
        + 32  // mint
        + 1   // side
        + 4   // position_index
        + 8   // collateral_amount
        + 8   // debt_amount
        + 8   // accrued_interest
        + 8   // last_slot
        + 1   // bump
        + 1; // vault_bump
}
