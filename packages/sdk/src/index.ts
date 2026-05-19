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
  getHolders,
  getMessages,
  getLendingInfo,
  getLoanPosition,
  getAllLoanPositions,
  getShortPosition,
  getVault,
  getVaultForWallet,
  getVaultWalletLink,
  getUserStats,
  getProtocolTreasuryState,
  getTreasuryState,
} from './tokens'

// quotes
export { getBuyQuote, getSellQuote, getBorrowQuote } from './quotes'

// transaction builders
export {
  buildBuyTransaction,
  buildDirectBuyTransaction,
  sendBuy,
  sendDirectBuy,
  sendCreateToken,
  buildSellTransaction,
  buildCreateTokenTransaction,
  buildStarTransaction,
  buildMigrateTransaction,
  buildBorrowTransaction,
  buildRepayTransaction,
  buildLiquidateTransaction,
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
} from './transactions'

// ephemeral Agent
export { createEphemeralAgent } from './ephemeral'
export type { EphemeralAgent } from './ephemeral'

// SAID Protocol
export { verifySaid, confirmTransaction } from './said'

// types
export type {
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
  StarParams,
  MigrateParams,
  TransactionResult,
  BuyTransactionResult,
  CreateTokenResult,
  BorrowParams,
  RepayParams,
  LiquidateParams,
  ClaimProtocolRewardsParams,
  ReclaimParams,
  LendingInfo,
  LoanPositionInfo,
  LoanPositionWithKey,
  AllLoanPositionsResult,
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
  ShortPositionInfo,
  OpenShortParams,
  CloseShortParams,
  LiquidateShortParams,
} from './types'

// constants (for advanced usage)
export {
  PROGRAM_ID,
  LAMPORTS_PER_SOL,
  TOKEN_MULTIPLIER,
  TOTAL_SUPPLY,
  LEGACY_MINTS,
  TOKEN_2022_PROGRAM_ID,
  PROTOCOL_TREASURY_SEED,
} from './constants'

// PDA / account derivers (for advanced usage — e.g. reading vault-owned ATAs directly).
// for raw DeepPool PDA helpers, import from `deeppoolsdk` directly.
export {
  getTorchVaultPda,
  getVaultWalletLinkPda,
  getBondingCurvePda,
  getProtocolTreasuryPda,
  getTokenTreasuryPda,
  getTreasuryTokenAccount,
  getTorchConfigPda,
  getDeepPoolAccounts,
} from './program'
