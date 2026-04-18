pub const TOTAL_SUPPLY: u64 = 1_000_000_000_000_000;
pub const MAX_WALLET_TOKENS: u64 = 20_000_000_000_000;
pub const TREASURY_SOL_MAX_BPS: u16 = 1750;  // 17.5% at start
pub const TREASURY_SOL_MIN_BPS: u16 = 250;   // 2.5% at completion
pub const TREASURY_FEE_BPS: u16 = 0;
pub const DEV_WALLET_SHARE_BPS: u16 = 5000;  // 50% of protocol fee to dev, 50% to user rewards
pub const SELL_FEE_BPS: u16 = 0;
pub const BONDING_TARGET_LAMPORTS: u64 = 200_000_000_000;
pub const BONDING_TARGET_SPARK: u64 = 50_000_000_000;   // 50 SOL
pub const BONDING_TARGET_FLAME: u64 = 100_000_000_000;  // 100 SOL
pub const BONDING_TARGET_TORCH: u64 = 200_000_000_000;  // 200 SOL (default)
pub const VALID_BONDING_TARGETS: [u64; 2] = [
    BONDING_TARGET_FLAME,
    BONDING_TARGET_TORCH,
];

pub const TOKEN_DECIMALS: u8 = 6;
pub const INITIAL_VIRTUAL_SOL: u64 = 30_000_000_000;
pub const INITIAL_VIRTUAL_TOKENS: u64 = 107_300_000_000_000;
pub const TREASURY_LOCK_TOKENS: u64 = 300_000_000_000_000;
pub const CURVE_SUPPLY: u64 = 700_000_000_000_000;
pub const TREASURY_LOCK_SEED: &[u8] = b"treasury_lock";
pub const INITIAL_VIRTUAL_TOKENS_V27: u64 = 756_250_000_000_000;

pub fn initial_virtual_reserves(bonding_target: u64) -> (u64, u64) {
    match bonding_target {
        BONDING_TARGET_SPARK => (18_750_000_000, INITIAL_VIRTUAL_TOKENS_V27),   // 18.75 SOL
        BONDING_TARGET_FLAME => (37_500_000_000, INITIAL_VIRTUAL_TOKENS_V27),   // 37.5 SOL
        BONDING_TARGET_TORCH => (75_000_000_000, INITIAL_VIRTUAL_TOKENS_V27),   // 75 SOL
        _ => (INITIAL_VIRTUAL_SOL, INITIAL_VIRTUAL_TOKENS),                      // Legacy
    }
}

pub const PROTOCOL_FEE_BPS: u16 = 50;
pub const MIN_SOL_AMOUNT: u64 = 1_000_000;
pub const TRANSFER_FEE_BPS: u16 = 7;
pub const MAX_TRANSFER_FEE: u64 = u64::MAX; // Uncapped per Token-2022 spec; actual fee governed by TRANSFER_FEE_BPS
pub const GLOBAL_CONFIG_SEED: &[u8] = b"global_config";
pub const BONDING_CURVE_SEED: &[u8] = b"bonding_curve";
pub const TREASURY_SEED: &[u8] = b"treasury";
pub const USER_POSITION_SEED: &[u8] = b"user_position";
pub const USER_STATS_SEED: &[u8] = b"user_stats";
pub const INACTIVITY_PERIOD_SLOTS: u64 = 7 * 24 * 60 * 60 * 1000 / 400;
pub const EPOCH_DURATION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const MIN_RECLAIM_THRESHOLD: u64 = 10_000_000;
pub const MIGRATION_SEED: &[u8] = b"migration";
pub const STAR_RECORD_SEED: &[u8] = b"star_record";
pub const CREATOR_REWARD_THRESHOLD: u64 = 2000;
pub const MIN_MIGRATION_SOL: u64 = 1_500_000_000; // 1.5 SOL

// DeepPool Program ID: CcwF61GW14AcxCS4E2zedHXdFXy8x8GQPvfxZrs2x2eT
pub const DEEP_POOL_PROGRAM_ID: anchor_lang::prelude::Pubkey = deep_pool::ID;
pub const DEEP_POOL_POOL_SEED: &[u8] = b"deep_pool";
pub const DEEP_POOL_VAULT_SEED: &[u8] = b"pool_vault";
pub const DEEP_POOL_LP_MINT_SEED: &[u8] = b"pool_lp_mint";
pub const TORCH_CONFIG_SEED: &[u8] = b"torch_config";

pub const STAR_COST_LAMPORTS: u64 = 20_000_000;
pub const CREATOR_FEE_SHARE_BPS: u16 = 1500;
pub const CREATOR_SOL_MIN_BPS: u16 = 20;
pub const CREATOR_SOL_MAX_BPS: u16 = 100;
pub const DEX_BUYBACK_MIN_SLIPPAGE_BPS: u16 = 100;
pub const DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS: u64 = 2700;
pub const RATIO_PRECISION: u128 = 1_000_000_000;
pub const DEFAULT_SELL_THRESHOLD_BPS: u16 = 12000;
pub const DEFAULT_SELL_PERCENT_BPS: u16 = 1500;
pub const SELL_ALL_TOKEN_THRESHOLD: u64 = 1_000_000_000_000;
pub const PROTOCOL_TREASURY_SEED: &[u8] = b"protocol_treasury_v11";
pub const PROTOCOL_TREASURY_RESERVE_FLOOR: u64 = 0;
pub const MIN_EPOCH_VOLUME_ELIGIBILITY: u64 = 2_000_000_000;
pub const MIN_CLAIM_AMOUNT: u64 = 100_000_000;
pub const MAX_CLAIM_SHARE_BPS: u64 = 1_000;
pub const REVIVAL_THRESHOLD: u64 = INITIAL_VIRTUAL_SOL;
pub const COLLATERAL_VAULT_SEED: &[u8] = b"collateral_vault";
pub const LOAN_SEED: &[u8] = b"loan";
pub const DEFAULT_INTEREST_RATE_BPS: u16 = 200;
pub const DEFAULT_MAX_LTV_BPS: u16 = 5000;
pub const DEFAULT_LIQUIDATION_THRESHOLD_BPS: u16 = 6500;
pub const DEFAULT_LIQUIDATION_BONUS_BPS: u16 = 1000;
pub const DEFAULT_LIQUIDATION_CLOSE_BPS: u16 = 5000;
pub const DEFAULT_LENDING_UTILIZATION_CAP_BPS: u16 = 8000;
pub const MIN_BORROW_AMOUNT: u64 = 100_000_000;
pub const BORROW_SHARE_MULTIPLIER: u64 = 23; // Per-user cap: max borrow = lendable * (collateral / denominator) * multiplier
pub const EPOCH_DURATION_SLOTS: u64 = 7 * 24 * 60 * 60 * 1000 / 400; // ~7 days at 400ms/slot
pub const METADATA_POINTER_EXTENSION_SIZE: usize = 68;
pub const TOKEN_METADATA_FIXED_SIZE: usize = 80;
pub const EXTENSION_TLV_HEADER_SIZE: usize = 4;
pub const COMMUNITY_TOKEN_SENTINEL: u64 = u64::MAX; // Stored in Treasury.total_bought_back to flag community tokens (0% creator fees)
pub const TORCH_VAULT_SEED: &[u8] = b"torch_vault";
pub const VAULT_WALLET_LINK_SEED: &[u8] = b"vault_wallet";
pub const SHORT_SEED: &[u8] = b"short";
pub const SHORT_CONFIG_SEED: &[u8] = b"short_config";
/// Prevents dust positions that cost more in rent than they're worth
pub const MIN_SHORT_TOKENS: u64 = 1_000_000_000;
pub const MIN_POOL_SOL_LENDING: u64 = 5_000_000_000;
pub const MAX_PRICE_DEVIATION_BPS: u64 = 5000;

// Depth-based risk bands: pool SOL thresholds and corresponding max LTV (bps).
// More SOL in pool = harder to manipulate = higher LTV allowed.
pub const DEPTH_TIER_1: u64 = 50_000_000_000;   // 50 SOL
pub const DEPTH_TIER_2: u64 = 200_000_000_000;  // 200 SOL
pub const DEPTH_TIER_3: u64 = 500_000_000_000;  // 500 SOL
pub const DEPTH_LTV_0: u16 = 2500;  // < 50 SOL  → 25%
pub const DEPTH_LTV_1: u16 = 3500;  // 50-200 SOL → 35%
pub const DEPTH_LTV_2: u16 = 4500;  // 200-500 SOL → 45%
pub const DEPTH_LTV_3: u16 = 5000;  // 500+ SOL  → 50%
pub const SHORT_ENABLED_SENTINEL: u16 = u16::MAX; // Stored in Treasury.buyback_percent_bps to flag short selling enabled
