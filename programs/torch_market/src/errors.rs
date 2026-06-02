use anchor_lang::prelude::*;

#[error_code]
pub enum TorchMarketError {
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Slippage tolerance exceeded")]
    SlippageExceeded,
    #[msg("Would exceed 2% wallet cap")]
    MaxWalletExceeded,
    #[msg("Insufficient tokens in pool")]
    InsufficientTokens,
    #[msg("Insufficient SOL in pool")]
    InsufficientSol,
    #[msg("Bonding curve already complete")]
    BondingComplete,
    #[msg("Bonding curve not yet complete")]
    BondingNotComplete,
    #[msg("Already migrated")]
    AlreadyMigrated,
    #[msg("Invalid authority")]
    InvalidAuthority,
    #[msg("Amount too small")]
    AmountTooSmall,
    #[msg("Zero amount not allowed")]
    ZeroAmount,
    #[msg("Name too long")]
    NameTooLong,
    #[msg("Symbol too long")]
    SymbolTooLong,
    #[msg("URI too long")]
    UriTooLong,
    #[msg("Treasury has insufficient SOL")]
    InsufficientTreasury,
    #[msg("Token has already been reclaimed")]
    AlreadyReclaimed,
    #[msg("Token is still active, cannot reclaim yet")]
    TokenStillActive,
    #[msg("Token SOL balance below reclaim threshold")]
    BelowReclaimThreshold,
    #[msg("Epoch has not ended yet")]
    EpochNotComplete,
    #[msg("Already claimed rewards for this epoch")]
    AlreadyClaimed,
    #[msg("No rewards available to claim")]
    NoRewardsAvailable,
    #[msg("No volume recorded in claimable epoch")]
    NoVolumeInEpoch,
    #[msg("Token not migrated to DEX yet")]
    NotMigrated,
    #[msg("Insufficient treasury balance for migration fee")]
    InsufficientMigrationFee,
    #[msg("Invalid dev wallet address")]
    InvalidDevWallet,
    #[msg("Unauthorized - only authority can perform this action")]
    Unauthorized,
    #[msg("Baseline must be initialized before auto buyback")]
    BaselineNotInitialized,
    #[msg("Pool reserves cannot be zero")]
    ZeroPoolReserves,
    #[msg("Insufficient volume for protocol rewards (need >= 2 SOL/epoch)")]
    InsufficientVolumeForRewards,
    #[msg("Token has not been reclaimed - cannot contribute to revival")]
    TokenNotReclaimed,
    #[msg("Lending is not enabled for this token")]
    LendingNotEnabled,
    #[msg("Token must be migrated to DEX before lending")]
    LendingRequiresMigration,
    #[msg("Treasury lending capacity exhausted (utilization cap reached)")]
    LendingCapExceeded,
    #[msg("Borrow amount below minimum (0.1 SOL)")]
    BorrowTooSmall,
    #[msg("No active loan position")]
    NoActiveLoan,
    #[msg("Position is not liquidatable (LTV below threshold)")]
    NotLiquidatable,
    #[msg("Must provide collateral or borrow amount")]
    EmptyBorrowRequest,
    #[msg("Invalid pool account")]
    InvalidPoolAccount,
    #[msg("Insufficient vault balance")]
    InsufficientVaultBalance,
    #[msg("Unauthorized - only vault authority can perform this action")]
    VaultUnauthorized,
    #[msg("Wallet link does not point to the provided vault")]
    VaultWalletLinkMismatch,
    #[msg("Invalid pool vault account")]
    InvalidPoolVault,
    #[msg("Invalid bonding target: must be 50, 100, or 200 SOL")]
    InvalidBondingTarget,
    #[msg("Claim amount below minimum (0.1 SOL)")]
    ClaimBelowMinimum,
    #[msg("Short selling is not enabled for this token")]
    ShortNotEnabled,
    #[msg("Short position size below minimum (1,000 tokens)")]
    ShortTooSmall,
    #[msg("Short position is not liquidatable (LTV below threshold)")]
    ShortNotLiquidatable,
    #[msg("No active short position")]
    NoActiveShort,
    #[msg("Pool depth below minimum for margin operations")]
    PoolTooThin,
    // NOTE: append new variants here, NOT in the middle of the enum.
    // Inserting in the middle shifts the Anchor-generated error codes
    // (6000 + index) and breaks every test that asserts on specific codes
    // for downstream variants. Same applies to clients with hard-coded
    // error matching.
    #[msg("Lending unlocks once treasury reaches the activity threshold (see docs/lending-unlock.md)")]
    LendingNotYetUnlocked,
}
