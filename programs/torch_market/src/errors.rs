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

    #[msg("Insufficient user balance")]
    InsufficientUserBalance,

    #[msg("Bonding curve already complete")]
    BondingComplete,

    #[msg("Bonding curve not yet complete")]
    BondingNotComplete,

    #[msg("Already voted")]
    AlreadyVoted,

    #[msg("No tokens to vote")]
    NoTokensToVote,

    #[msg("Already migrated")]
    AlreadyMigrated,

    #[msg("Invalid authority")]
    InvalidAuthority,

    #[msg("Amount too small")]
    AmountTooSmall,

    #[msg("Protocol paused")]
    ProtocolPaused,

    #[msg("Zero amount not allowed")]
    ZeroAmount,

    #[msg("Name too long")]
    NameTooLong,

    #[msg("Symbol too long")]
    SymbolTooLong,

    #[msg("URI too long")]
    UriTooLong,

    #[msg("Buyback interval not elapsed")]
    BuybackTooSoon,

    #[msg("Treasury has insufficient SOL")]
    InsufficientTreasury,

    #[msg("Vote not finalized")]
    VoteNotFinalized,

    #[msg("This operation requires a Token-2022 mint")]
    NotToken2022,

    #[msg("Vote is required on first buy")]
    VoteRequired,

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

    #[msg("DEX pool creation failed")]
    PoolCreationFailed,

    #[msg("Insufficient treasury balance for migration fee")]
    InsufficientMigrationFee,

    #[msg("Cannot star yourself")]
    CannotStarSelf,

    #[msg("Invalid dev wallet address")]
    InvalidDevWallet,

    #[msg("Unauthorized - only authority can perform this action")]
    Unauthorized,

    #[msg("Baseline must be initialized before auto buyback")]
    BaselineNotInitialized,

    #[msg("Pool reserves cannot be zero")]
    ZeroPoolReserves,

    #[msg("Price above threshold - no buyback needed")]
    RatioAboveThreshold,

    #[msg("Treasury at reserve floor - cannot execute buyback")]
    AtReserveFloor,

    #[msg("Insufficient volume for protocol rewards (need >= 2 SOL/epoch)")]
    InsufficientVolumeForRewards,

    #[msg("Protocol treasury below reserve floor")]
    ProtocolTreasuryBelowFloor,

    #[msg("Protocol treasury already initialized")]
    ProtocolTreasuryAlreadyInitialized,

    #[msg("Token has not been reclaimed - cannot contribute to revival")]
    TokenNotReclaimed,

    #[msg("Lending is not enabled for this token")]
    LendingNotEnabled,

    #[msg("Token must be migrated to DEX before lending")]
    LendingRequiresMigration,

    #[msg("Loan-to-value ratio exceeds maximum")]
    LtvExceeded,

    #[msg("Treasury lending capacity exhausted (utilization cap reached)")]
    LendingCapExceeded,

    #[msg("Per-user borrow cap exceeded (max 5x collateral share of supply)")]
    UserBorrowCapExceeded,

    #[msg("Borrow amount below minimum (0.1 SOL)")]
    BorrowTooSmall,

    #[msg("No active loan position")]
    NoActiveLoan,

    #[msg("Position is not liquidatable (LTV below threshold)")]
    NotLiquidatable,

    #[msg("Must provide collateral or borrow amount")]
    EmptyBorrowRequest,

    #[msg("Repay amount exceeds total owed")]
    RepayExceedsDebt,

    #[msg("Invalid pool account")]
    InvalidPoolAccount,

    #[msg("Insufficient vault balance")]
    InsufficientVaultBalance,

    #[msg("Unauthorized - only vault authority can perform this action")]
    VaultUnauthorized,

    #[msg("Wallet is not linked to any vault")]
    WalletNotLinked,

    #[msg("Wallet link does not point to the provided vault")]
    VaultWalletLinkMismatch,

    #[msg("Invalid pool vault account")]
    InvalidPoolVault,

    #[msg("Invalid bonding target: must be 50, 100, or 200 SOL")]
    InvalidBondingTarget,

    #[msg("Invalid token account - must be treasury lock's Token-2022 ATA")]
    InvalidTokenAccount,

    #[msg("Claim amount below minimum (0.1 SOL)")]
    ClaimBelowMinimum,

    #[msg("Short selling is not enabled for this token")]
    ShortNotEnabled,

    #[msg("Short position size below minimum (1,000 tokens)")]
    ShortTooSmall,

    #[msg("Token lending capacity exhausted (short utilization cap reached)")]
    ShortCapExceeded,

    #[msg("Per-user short cap exceeded")]
    UserShortCapExceeded,

    #[msg("Short position is not liquidatable (LTV below threshold)")]
    ShortNotLiquidatable,

    #[msg("No active short position")]
    NoActiveShort,

    #[msg("Treasury has insufficient tokens for short lending")]
    InsufficientTreasuryTokens,

    #[msg("Short selling already enabled for this token")]
    ShortAlreadyEnabled,

    #[msg("Pool depth below minimum for margin operations")]
    PoolTooThin,

    #[msg("Pool price deviates too far from baseline")]
    PriceDeviationTooHigh,
}
