/**
 * @torch-market/sdk
 *
 * AI agent toolkit for Solana fair-launch tokens.
 * Usage:
 *   import { getTokens, buildBuyTransaction } from "@torch-market/sdk";
 *   const connection = new Connection("https://api.mainnet-beta.solana.com");
 *   const tokens = await getTokens(connection);
 *   const tx = await buildBuyTransaction(connection, { mint, buyer, amount_sol: 100_000_000 });
 */

// token data
export {
  getTokens,
  getTokensPage,
  getToken,
  getTokenMetadata,
  fetchMintsMetadata,
  getHolders,
  getMessages,
  getLendingInfo,
  getPosition,
  getAllPositions,
  getVault,
  getVaultForWallet,
  getVaultWalletLink,
  getUserStats,
  getProtocolTreasuryState,
  getTreasuryState,
} from './tokens'

// quotes
export { getBuyQuote, getSellQuote, getBorrowQuote } from './quotes'
export { TRANSFER_FEE_BPS, grossUpForTransferFee } from './tokens'

// indexer-only getters + wire types
export { getTrades, getSwaps, getCandles, getUserPnl, getLiquidations } from './indexer'
export type {
  IndexerMarketStatus,
  IndexerMarketTier,
  IndexerPositionHealth,
  IndexerPositionSide,
  IndexerPositionEventKind,
  IndexerMarketRow,
  IndexerTradeRow,
  IndexerSwapRow,
  IndexerMessageRow,
  IndexerPositionRow,
  IndexerPositionEventRow,
  IndexerCandle,
  UserPnlByMint,
  UserPnlSummary,
  TradeHistoryQuery,
  SwapsQuery,
  CandlesQuery,
  LiquidationsQuery,
} from './indexer'

// priority fees (compute-unit price) — applied to every SDK-built transaction
export {
  setPriorityFeeMicroLamports,
  getPriorityFeeMicroLamports,
  estimatePriorityFee,
  // [prompt-008 F-3] shared slippage haircut — keeps UI "min guaranteed" == signed floor
  applySlippageBps,
} from './transactions'

// transaction builders
export {
  buildBuyTransaction,
  buildDirectBuyTransaction,
  sendBuy,
  sendDirectBuy,
  sendCreateToken,
  buildSellTransaction,
  buildCreateTokenTransaction,
  buildMigrateTransaction,
  buildClaimProtocolRewardsTransaction,
  buildReclaimFailedTokenTransaction,
  buildCreateVaultTransaction,
  buildDepositVaultTransaction,
  buildWithdrawVaultTransaction,
  buildLinkWalletTransaction,
  buildUnlinkWalletTransaction,
  buildTransferAuthorityTransaction,
  buildWithdrawTokensTransaction,
  buildHarvestFeesTransaction,
  buildSwapFeesToSolTransaction,
  buildAdvanceProtocolEpochTransaction,
  buildOpenShortTransaction,
  buildCloseShortTransaction,
  buildLiquidateShortTransaction,
  buildOpenLongTransaction,
  buildCloseLongTransaction,
  buildLiquidateLongTransaction,
} from './transactions'

// ephemeral Agent
export { createEphemeralAgent } from './ephemeral'
export type { EphemeralAgent } from './ephemeral'

// SAID Protocol
export { verifySaid, confirmTransaction } from './said'

// types
export type {
  ReadOptions,
  TokenStatus,
  TokenSummary,
  TokenDetail,
  TokenSortOption,
  TokenStatusFilter,
  TokenListParams,
  TokenListResult,
  TokenPageParams,
  TokenPageResult,
  Holder,
  HoldersResult,
  BuyQuoteResult,
  SellQuoteResult,
  BorrowQuoteResult,
  BuyParams,
  DirectBuyParams,
  SellParams,
  CreateTokenParams,
  MigrateParams,
  TransactionResult,
  BuyTransactionResult,
  CreateTokenResult,
  ClaimProtocolRewardsParams,
  ReclaimParams,
  LendingInfo,
  PositionSide,
  PositionInfo,
  PositionWithKey,
  AllPositionsResult,
  TokenMessage,
  MessagesResult,
  SaidVerification,
  ConfirmResult,
  WalletAdapter,
  VaultInfo,
  VaultWalletLinkInfo,
  UserStatsInfo,
  ProtocolTreasuryInfo,
  TreasuryInfo,
  CreateVaultParams,
  DepositVaultParams,
  WithdrawVaultParams,
  LinkWalletParams,
  UnlinkWalletParams,
  TransferAuthorityParams,
  WithdrawTokensParams,
  HarvestFeesParams,
  SwapFeesToSolParams,
  AdvanceProtocolEpochParams,
  TokenMetadataResult,
  OpenShortParams,
  CloseShortParams,
  LiquidateShortParams,
  OpenLongParams,
  CloseLongParams,
  LiquidateLongParams,
} from './types'

// constants (for advanced usage)
export {
  PROGRAM_ID,
  LAMPORTS_PER_SOL,
  TOKEN_MULTIPLIER,
  TOKEN_DECIMALS,
  TOTAL_SUPPLY,
  LEGACY_MINTS,
  TOKEN_2022_PROGRAM_ID,
  PROTOCOL_TREASURY_SEED,
  BONDING_CURVE_SEED,
  TREASURY_SEED,
  USER_POSITION_SEED,
  USER_STATS_SEED,
  TREASURY_LOCK_SEED,
  POSITION_SEED,
  TREASURY_SOL_VAULT_SEED,
} from './constants'

// PDA / account derivers (for advanced usage — e.g. reading vault-owned ATAs directly).
// for raw DeepPool PDA helpers, import from `deeppoolsdk` directly.
export {
  getTorchVaultPda,
  getVaultWalletLinkPda,
  getBondingCurvePda,
  getProtocolTreasuryPda,
  getTokenTreasuryPda,
  getTreasurySolVaultPda,
  getTreasuryTokenAccount,
  getTreasuryLockPda,
  getUserStatsPda,
  getUserPositionPda,
  getPositionPda,
  getShortVaultPda,
  getLongSolVaultPda,
  getPositionTokenVault,
  getTorchConfigPda,
  getDeepPoolAccounts,
  getDeepPoolPda,
  decodeString,
  calculatePrice,
  calculateBondingProgress,
} from './program'

// account interfaces (for callers decoding on-chain accounts directly)
export type {
  BondingCurve,
  Treasury as TreasuryAccount,
  ProtocolTreasury as ProtocolTreasuryAccount,
  UserStats as UserStatsAccount,
  Position as PositionAccount,
} from './program'
export type { MintMetadata } from './tokens'
export * from './feed'
