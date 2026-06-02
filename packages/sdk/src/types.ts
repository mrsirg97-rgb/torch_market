/**
 * Torch Market SDK Types
 */

import { PublicKey, VersionedTransaction, Keypair } from '@solana/web3.js'

// ============================================================================
// Indexer Integration
// ============================================================================
//
// Pass `indexer` to opt into the indexer-first read path; on any failure the
// call silently falls back to RPC. Without it, every read goes straight to
// the chain.
//
// Two getters (`getTrades`, `getCandles`) are indexer-only — RPC can't
// aggregate historical events in any reasonable time. They throw if `indexer`
// is omitted.

export interface ReadOptions {
  /** Base URL of a running torch-indexer (e.g. http://localhost:8080). */
  indexer?: string
}

// ============================================================================
// Token Types
// ============================================================================

export type TokenStatus = 'bonding' | 'complete' | 'migrated' | 'reclaimed'

export interface TokenSummary {
  mint: string
  name: string
  symbol: string
  status: TokenStatus
  price_sol: number
  market_cap_sol: number
  progress_percent: number
  holders: number | null
  created_at: number
  last_activity_at: number
  // Optional enrichment fields populated when an indexer is available.
  // The RPC fallback leaves these undefined; consumers can backfill via
  // per-mint metadata fetches (see useTokens.ts enrichment loop).
  image?: string
  creator?: string
  bonding_target?: number
  tier?: 'spark' | 'flame' | 'torch'
}

export interface TokenDetail {
  mint: string
  name: string
  symbol: string
  description?: string
  image?: string
  status: TokenStatus
  price_sol: number
  price_usd?: number
  market_cap_sol: number
  market_cap_usd?: number
  progress_percent: number
  sol_raised: number
  sol_target: number
  total_supply: number
  circulating_supply: number
  tokens_in_curve: number
  tokens_burned: number
  treasury_sol_balance: number
  treasury_token_balance: number
  creator: string
  holders: number | null
  stars: number
  created_at: number
  last_activity_at: number
  twitter?: string
  telegram?: string
  website?: string
  creator_verified?: boolean
  creator_trust_tier?: 'high' | 'medium' | 'low' | null
  creator_said_name?: string
  creator_badge_url?: string
  warnings?: string[]
}

// ============================================================================
// List Params
// ============================================================================

export type TokenSortOption = 'newest' | 'volume' | 'marketcap'
export type TokenStatusFilter = 'bonding' | 'complete' | 'migrated' | 'reclaimed' | 'all'

export interface TokenListParams {
  limit?: number
  offset?: number
  status?: TokenStatusFilter
  sort?: TokenSortOption
}

export interface TokenListResult {
  tokens: TokenSummary[]
  total: number
  limit: number
  offset: number
}

// Pagination params for getTokensPage (uses RPC getProgramAccountsV2).
// limit: RPC page size 1-10000 (default 10000).
// paginationKey: opaque cursor from previous response; null/undefined for first page.
// changedSinceSlot: only return accounts modified at or after this slot (for delta polling).
export interface TokenPageParams {
  limit?: number
  paginationKey?: string | null
  changedSinceSlot?: number
}

// One page of tokens from getTokensPage.
// paginationKey: cursor for next page, or null when done.
// currentSlot: slot this response is current at — pass as changedSinceSlot on next poll.
export interface TokenPageResult {
  tokens: TokenSummary[]
  paginationKey: string | null
  currentSlot: number
}

// ============================================================================
// Holders
// ============================================================================

export interface Holder {
  address: string
  balance: number
  percentage: number
}

export interface HoldersResult {
  holders: Holder[]
  total_holders: number
}

// ============================================================================
// Quotes
// ============================================================================

export interface BuyQuoteResult {
  input_sol: number
  output_tokens: number
  tokens_to_user: number
  protocol_fee_sol: number
  price_per_token_sol: number
  price_impact_percent: number
  min_output_tokens: number
  /** Where this quote came from: bonding curve or DeepPool */
  source: 'bonding' | 'dex'
}

export interface SellQuoteResult {
  input_tokens: number
  output_sol: number
  protocol_fee_sol: number
  price_per_token_sol: number
  price_impact_percent: number
  min_output_sol: number
  /** Where this quote came from: bonding curve or DeepPool */
  source: 'bonding' | 'dex'
}

// [V21] Open-long quote: max SOL borrowable against token collateral. (V20's
// per-user formula + lending-unlock gate were removed; the on-chain clamp is
// LTV + treasury lendable headroom.)
export interface BorrowQuoteResult {
  /** Max SOL borrowable against the posted token collateral (lamports). */
  max_borrow_sol: number
  /** SOL value of the posted token collateral (lamports). */
  collateral_value_sol: number
  /** LTV-bound cap: collateral_value × max_ltv (lamports). */
  ltv_max_sol: number
  /** Treasury lendable headroom: vault × utilization_cap − already lent (lamports). */
  pool_available_sol: number
  /** [V21] Rail-2 size cap: ρ_max × pool SOL — the max debt value on this pool (lamports). */
  size_cap_sol: number
  interest_rate_bps: number
  /** Effective max LTV actually applied = min(depth curve, treasury ceiling). */
  max_ltv_bps: number
  liquidation_threshold_bps: number
}

// ============================================================================
// Vault Types (V2.0)
// ============================================================================

export interface VaultInfo {
  address: string
  creator: string
  authority: string
  sol_balance: number
  total_deposited: number
  total_withdrawn: number
  total_spent: number
  total_received: number
  linked_wallets: number
  created_at: number
}

export interface VaultWalletLinkInfo {
  address: string
  vault: string
  wallet: string
  linked_at: number
}

export interface UserStatsInfo {
  address: string
  user: string
  /** Lifetime trading volume in SOL */
  total_volume_sol: number
  /** Volume attributed to the current epoch (SOL) */
  volume_current_epoch_sol: number
  /** Volume attributed to the previous epoch — claim eligibility is against this (SOL) */
  volume_previous_epoch_sol: number
  /** Epoch number that was most recently claimed *for* (not *when*). The on-chain
   *  claim instruction settles the prior epoch's volume, so this is typically
   *  `ProtocolTreasury.current_epoch - 1` at the time of the claim. */
  last_epoch_claimed: number
  /** Lifetime rewards claimed (SOL) */
  total_rewards_claimed_sol: number
  /** Epoch number at which volume was last recorded */
  last_volume_epoch: number
}

// [V21] Treasury is accounting-only; lendable SOL lives in the System-owned
// treasury_sol_vault PDA. Star/creator-reward + V20 single-loan counters are
// gone; long and short are tracked separately.
export interface TreasuryInfo {
  address: string
  /** The bonding curve PDA this treasury is associated with */
  bonding_curve: string
  mint: string
  /** Lendable SOL custodied in the System-owned treasury_sol_vault PDA (SOL). */
  treasury_sol_vault_sol: number
  /** True if this token was created as a community token (0% creator fees) */
  is_community_token: boolean
  /** Harvested Token-2022 transfer fees swapped to SOL — cumulative (SOL) */
  harvested_fees_sol: number
  /** Baseline pool SOL reserves captured at migration (lamports) */
  baseline_sol_reserves: number
  /** Baseline pool token reserves captured at migration (token base units) */
  baseline_token_reserves: number
  baseline_initialized: boolean
  /** Whether short selling is enabled for this token */
  short_selling_enabled: boolean
  /** Whether lending (longs) is enabled for this token */
  lending_enabled: boolean
  /** Slot of the last treasury fee-to-SOL swap */
  last_buyback_slot: number
  // [V21] Leverage accounting.
  total_tokens_lent: number
  active_shorts: number
  total_sol_lent_to_longs: number
  active_longs: number
  total_token_collateral_locked: number
}

export interface ProtocolTreasuryInfo {
  address: string
  authority: string
  /** Current SOL balance in the protocol treasury */
  current_balance_sol: number
  /** Reserve floor kept across epochs (SOL) */
  reserve_floor_sol: number
  /** Lifetime fees received (SOL) */
  total_fees_received_sol: number
  /** Lifetime SOL distributed to claimers */
  total_distributed_sol: number
  current_epoch: number
  /** Unix timestamp of the last epoch rollover */
  last_epoch_ts: number
  /** Aggregate trading volume across all users in the current epoch (SOL) */
  total_volume_current_epoch_sol: number
  /** Aggregate volume in the previous epoch — denominator for reward shares (SOL) */
  total_volume_previous_epoch_sol: number
  /** Amount currently available to distribute this epoch (SOL) */
  distributable_amount_sol: number
}

// ============================================================================
// Vault Params (V2.0)
// ============================================================================

export interface CreateVaultParams {
  creator: string
}

export interface DepositVaultParams {
  depositor: string
  vault_creator: string
  amount_sol: number
}

export interface WithdrawVaultParams {
  authority: string
  vault_creator: string
  amount_sol: number
}

export interface LinkWalletParams {
  authority: string
  vault_creator: string
  wallet_to_link: string
}

export interface UnlinkWalletParams {
  authority: string
  vault_creator: string
  wallet_to_unlink: string
}

export interface TransferAuthorityParams {
  authority: string
  vault_creator: string
  new_authority: string
}

export interface WithdrawTokensParams {
  authority: string
  vault_creator: string
  mint: string
  destination: string
  amount: number
}

// ============================================================================
// Transaction Params
// ============================================================================

export interface BuyParams {
  mint: string
  buyer: string
  amount_sol: number
  slippage_bps?: number
  message?: string
  vault: string // vault creator pubkey; vault pays for the buy
  quote?: BuyQuoteResult // pre-fetched quote; if provided, skips internal fetch and routes bonding vs DEX by quote.source
}

export interface DirectBuyParams {
  mint: string
  buyer: string
  amount_sol: number
  slippage_bps?: number
  message?: string
  quote?: BuyQuoteResult // pre-fetched quote; if provided, skips internal fetch
}

export interface SellParams {
  mint: string
  seller: string
  amount_tokens: number
  slippage_bps?: number
  message?: string
  /** Vault creator pubkey. SOL goes to vault, tokens sold from vault ATA. */
  vault?: string
  /** Pre-fetched quote from getSellQuote. If provided, skips internal quote fetch
   *  and uses quote.source to route bonding vs DEX. */
  quote?: SellQuoteResult
}

export interface CreateTokenParams {
  creator: string
  name: string
  symbol: string
  metadata_uri: string
  /** [V23] Bonding target in lamports. 0 or omitted = default 200 SOL. */
  sol_target?: number
  /** [V35] Community token: 0% creator fees, all to treasury. Default true. */
  community_token?: boolean
}

// ============================================================================
// Migration Params (V26)
// ============================================================================

export interface MigrateParams {
  /** Token mint address */
  mint: string
  /** Wallet signing the transaction. Fronts ~0.003 SOL for account rent,
   *  reimbursed by treasury in the same transaction. */
  payer: string
}

// ============================================================================
// Vault Swap Params (V19)
// ============================================================================

export interface VaultSwapParams {
  /** Token mint address */
  mint: string
  /** Controller wallet (linked to vault, signs the tx) */
  signer: string
  /** Vault creator pubkey (for PDA derivation) */
  vault_creator: string
  /** Input amount (lamports for buy, token base units for sell) */
  amount_in: number
  /** Minimum output for slippage protection */
  minimum_amount_out: number
  /** true = SOL→Token (buy), false = Token→SOL (sell) */
  is_buy: boolean
  /** Optional message bundled as SPL Memo instruction (max 500 chars) */
  message?: string
}

// ============================================================================
// Treasury Crank Params
// ============================================================================

export interface HarvestFeesParams {
  /** Token mint address */
  mint: string
  /** Payer wallet (permissionless — anyone can trigger) */
  payer: string
  /** Optional list of token account addresses to harvest from.
   *  If omitted, the SDK auto-discovers accounts with withheld fees. */
  sources?: string[]
}

export interface AdvanceProtocolEpochParams {
  /** Payer wallet (permissionless — anyone can trigger) */
  payer: string
}

export interface SwapFeesToSolParams {
  /** Token mint address */
  mint: string
  /** Payer wallet (permissionless — anyone can trigger) */
  payer: string
  /** Minimum SOL out from the swap (slippage protection, default 1) */
  minimum_amount_out?: number
  /** Bundle harvest_fees in the same transaction (default true) */
  harvest?: boolean
  /** Optional list of token account addresses to harvest from.
   *  Only used when harvest=true. If omitted, auto-discovers. */
  sources?: string[]
}

// ============================================================================
// V29: Token Metadata
// ============================================================================

export interface TokenMetadataResult {
  /** Token name from on-chain metadata */
  name: string
  /** Token symbol from on-chain metadata */
  symbol: string
  /** Token metadata URI */
  uri: string
  /** Mint address */
  mint: string
}

// ============================================================================
// Wallet Adapter
// ============================================================================

/**
 * Minimal wallet interface for signAndSendTransaction flows.
 * Compatible with Phantom, Backpack, and other Solana wallets that
 * support atomic sign-and-send.
 */
export interface WalletAdapter {
  publicKey: PublicKey
  signAndSendTransaction: (tx: VersionedTransaction) => Promise<{ signature: string }>
}

// ============================================================================
// Transaction Results
// ============================================================================

export interface TransactionResult {
  transaction: VersionedTransaction
  /** Additional transactions when a single tx exceeds the size limit.
   *  When present, send all transactions in order: transaction first, then these. */
  additionalTransactions?: VersionedTransaction[]
  message: string
}

export interface BuyTransactionResult extends TransactionResult {
  /** [V28] Follow-up migration transaction. Present when this buy completes
   *  bonding. Send immediately after the buy tx succeeds. The payer fronts
   *  ~0.003 SOL for account rent, reimbursed by treasury in the same tx.
   *  If the caller can't afford it or it fails, anyone can trigger migration
   *  later via buildMigrateTransaction. */
  migrationTransaction?: VersionedTransaction
}

export interface CreateTokenResult extends TransactionResult {
  mint: PublicKey
  mintKeypair: Keypair
}

// ============================================================================
// Lending Params (V2.4)
// ============================================================================

export interface ClaimProtocolRewardsParams {
  user: string
  /** Vault creator pubkey. Claimed SOL goes to vault instead of user. */
  vault?: string
}

export interface ReclaimParams {
  /** Payer/caller wallet (permissionless — anyone can call) */
  payer: string
  /** Token mint to reclaim */
  mint: string
}

// ============================================================================
// [V21] Leverage params — unified closed-loop long + short positions.
//
// Open: borrow size = collateral × LTV, then clamped to protocol/user caps (the
// SDK has no "borrow size" knob — the UI shows the clamped size pre-sign). Close
// is fractional via `repay_fraction_bps` (10000 = full). A `vault` creator
// pubkey routes the matching `*_via_vault` variant (position owner = TorchVault
// PDA). `position_index` lets a wallet hold multiple positions per (mint, side).
// ============================================================================

export type PositionSide = 'long' | 'short'

export interface OpenShortParams {
  mint: string
  shorter: string
  /** Position slot (default 0). A wallet can hold many shorts on one mint. */
  position_index?: number
  /** SOL collateral, lamports. Borrowed tokens = collateral × LTV (clamped). */
  collateral: number
  /** Min SOL out from selling the borrowed tokens (slippage guard; 0 = none). */
  min_out?: number
  /** Vault creator pubkey → routes open_short_via_vault. */
  vault?: string
}

export interface OpenLongParams {
  mint: string
  borrower: string
  /** Position slot (default 0). */
  position_index?: number
  /** Token collateral (6 decimals). Borrowed SOL = collateral × LTV (clamped). */
  collateral: number
  /** Min tokens out from the atomic buy (slippage guard; 0 = none). */
  min_out?: number
  /** Vault creator pubkey → routes open_long_via_vault. */
  vault?: string
}

export interface CloseShortParams {
  mint: string
  shorter: string
  position_index?: number
  /** Fraction of the position to close, in bps (10000 = full close; default). */
  repay_fraction_bps?: number
  /** Min surplus SOL returned to the user (slippage guard). */
  min_surplus_sol_out?: number
  vault?: string
}

export interface CloseLongParams {
  mint: string
  borrower: string
  position_index?: number
  repay_fraction_bps?: number
  min_surplus_sol_out?: number
  vault?: string
}

export interface LiquidateShortParams {
  mint: string
  liquidator: string
  borrower: string
  position_index?: number
  /** Set when the position owner is a TorchVault (routes liquidate_short_via_vault). */
  vault?: string
  /**
   * [V21] Auto-acquire the cover tokens. A short's debt is token-denominated, so
   * the liquidator must SUPPLY tokens to cover it. When true (default), the SDK
   * prepends a DeepPool buy that funds the liquidator's ATA with the cover
   * amount, so the liquidator only needs SOL — the seize (debt + bonus) repays
   * it. Set false if the liquidator already warehouses enough tokens.
   */
  acquire_cover_tokens?: boolean
}

export interface LiquidateLongParams {
  mint: string
  liquidator: string
  borrower: string
  position_index?: number
  /** Set when the position owner is a TorchVault (routes liquidate_long_via_vault). */
  vault?: string
}

// ============================================================================
// [V21] Lending / leverage info — treasury-level rates + custody snapshot.
// Lendable SOL lives in the System-owned treasury_sol_vault; the Treasury data
// account holds long/short accounting only.
// ============================================================================

export interface LendingInfo {
  interest_rate_bps: number
  max_ltv_bps: number
  liquidation_threshold_bps: number
  liquidation_bonus_bps: number
  liquidation_close_bps: number
  utilization_cap_bps: number
  lending_enabled: boolean
  short_selling_enabled: boolean
  /** Lendable SOL custodied in the System-owned treasury_sol_vault (lamports). */
  treasury_sol_vault_lamports: number
  /** V21 long-side accounting (from the Treasury data account). */
  total_sol_lent_to_longs: number
  active_longs: number
  /** V21 short-side accounting. */
  total_tokens_lent: number
  active_shorts: number
  total_token_collateral_locked: number
  warnings?: string[]
}

// ============================================================================
// [V21] Position results — unified long + short (replaces Loan*/Short* infos).
// Unit-by-side: short → collateral = SOL, debt = tokens; long → collateral =
// tokens, debt = SOL.
// ============================================================================

export interface PositionInfo {
  side: PositionSide
  position_index: number
  /** Unit-by-side: short → SOL lamports; long → tokens (6 decimals). */
  collateral_amount: number
  /** Unit-by-side: short → tokens; long → SOL lamports. */
  debt_amount: number
  /** Accrued interest projected to the current slot (token units for short, SOL
   *  for long), matching what the program writes at the next touch. */
  accrued_interest: number
  /** Raw stored accrued_interest from the Position account (as of last_slot). */
  accrued_interest_stored: number
  /** Slot at which `accrued_interest_stored` was last written. */
  last_update_slot: number
  /** Debt incl. projected interest (token units for short, SOL lamports for long). */
  total_owed: number
  /** SOL value of the debt (null if pool price unavailable). */
  debt_value_sol: number | null
  /** Current LTV in bps off the pool mark (null if price unavailable). */
  current_ltv_bps: number | null
  health: 'healthy' | 'at_risk' | 'liquidatable' | 'none'
  /** True when the position is owned by a TorchVault PDA (opened via_vault). */
  owner_is_vault: boolean
  warnings?: string[]
}

export interface PositionWithKey extends PositionInfo {
  /** Position owner — wallet (direct) or TorchVault PDA (via_vault). */
  owner: string
}

export interface AllPositionsResult {
  positions: PositionWithKey[]
  pool_price_sol: number | null
}

// ============================================================================
// Messages
// ============================================================================

export interface TokenMessage {
  signature: string
  memo: string
  sender: string
  timestamp: number
  sender_verified?: boolean
  sender_trust_tier?: 'high' | 'medium' | 'low' | null
  sender_said_name?: string
  sender_badge_url?: string
}

export interface MessagesResult {
  messages: TokenMessage[]
  total: number
}

// ============================================================================
// SAID
// ============================================================================

export interface SaidVerification {
  verified: boolean
  trustTier: 'high' | 'medium' | 'low' | null
  name?: string
}

export interface ConfirmResult {
  confirmed: boolean
  event_type: 'token_launch' | 'trade_complete' | 'unknown'
}
