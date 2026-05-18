import { Program, AnchorProvider, BN } from '@coral-xyz/anchor'
import { PublicKey } from '@solana/web3.js'
import {
  PROGRAM_ID,
  GLOBAL_CONFIG_SEED,
  BONDING_CURVE_SEED,
  TREASURY_SEED,
  USER_POSITION_SEED,
  PROTOCOL_TREASURY_SEED,
  USER_STATS_SEED,
  STAR_RECORD_SEED,
  LOAN_SEED,
  COLLATERAL_VAULT_SEED,
  TORCH_VAULT_SEED,
  TORCH_VAULT_SOL_SEED,
  VAULT_WALLET_LINK_SEED,
  TREASURY_LOCK_SEED,
  SHORT_SEED,
  SHORT_CONFIG_SEED,
  TOKEN_2022_PROGRAM_ID,
} from './constants'
import { getAssociatedTokenAddressSync } from '@solana/spl-token'

// re-export program ID for convenience
export { PROGRAM_ID }
import idl from './torch_market.json'

// types from IDL (snake_case to match Anchor decoding). v20 layout — pre-v20
// vote_* fields and sentinel-repurposed counters have been removed.
export interface BondingCurve {
  mint: PublicKey
  creator: PublicKey
  virtual_sol_reserves: BN
  virtual_token_reserves: BN
  real_sol_reserves: BN
  real_token_reserves: BN
  bonding_complete: boolean
  bonding_complete_slot: BN
  migrated: boolean
  last_activity_slot: BN
  reclaimed: boolean
  bump: number
  treasury_bump: number
  bonding_target: BN
}

export interface GlobalConfig {
  authority: PublicKey
  treasury: PublicKey
  dev_wallet: PublicKey
  protocol_fee_bps: number
  paused: boolean
  total_tokens_launched: BN
  total_volume_sol: BN
  bump: number
}

export interface Treasury {
  bonding_curve: PublicKey
  mint: PublicKey
  sol_balance: BN
  is_community_token: boolean
  short_collateral_reserved: BN
  last_buyback_slot: BN
  harvested_fees: BN
  bump: number
  baseline_sol_reserves: BN
  baseline_token_reserves: BN
  short_selling_enabled: boolean
  min_buyback_interval_slots: BN
  baseline_initialized: boolean
  total_stars: BN
  star_sol_balance: BN
  creator_paid_out: boolean
  total_sol_lent: BN
  total_collateral_locked: BN
  active_loans: BN
  total_interest_collected: BN
  lending_enabled: boolean
  interest_rate_bps: number
  max_ltv_bps: number
  liquidation_threshold_bps: number
  liquidation_bonus_bps: number
  liquidation_close_bps: number
  lending_utilization_cap_bps: number
}

export interface TorchVault {
  creator: PublicKey
  authority: PublicKey
  sol_balance: BN
  total_deposited: BN
  total_withdrawn: BN
  total_spent: BN
  total_received: BN
  linked_wallets: number
  created_at: BN
  bump: number
}

export interface VaultWalletLink {
  vault: PublicKey
  wallet: PublicKey
  linked_at: BN
  bump: number
}

export interface LoanPosition {
  user: PublicKey
  mint: PublicKey
  collateral_amount: BN
  borrowed_amount: BN
  accrued_interest: BN
  last_update_slot: BN
  bump: number
}

export interface ShortPosition {
  user: PublicKey
  mint: PublicKey
  sol_collateral: BN
  tokens_borrowed: BN
  accrued_interest: BN
  last_update_slot: BN
  bump: number
}

export interface ShortConfig {
  mint: PublicKey
  total_tokens_lent: BN
  active_positions: BN
  total_interest_collected: BN
  bump: number
}

export interface UserStats {
  user: PublicKey
  total_volume: BN
  volume_current_epoch: BN
  volume_previous_epoch: BN
  last_epoch_claimed: BN
  total_rewards_claimed: BN
  last_volume_epoch: BN
  bump: number
}

export interface ProtocolTreasury {
  authority: PublicKey
  current_balance: BN
  reserve_floor: BN
  total_fees_received: BN
  total_distributed: BN
  current_epoch: BN
  last_epoch_ts: BN
  total_volume_current_epoch: BN
  total_volume_previous_epoch: BN
  distributable_amount: BN
  bump: number
}

// treasury SOL rate decays linearly from 17.5% at bonding start to 2.5% at completion
const TREASURY_SOL_MAX_BPS = 1750
const TREASURY_SOL_MIN_BPS = 250

// creator SOL share grows from 0.2% → 1% during bonding (carved from treasury rate)
const CREATOR_SOL_MIN_BPS = 20
const CREATOR_SOL_MAX_BPS = 100

export const decodeString = (bytes: number[]): string =>
  Buffer.from(bytes).toString('utf8').replace(/\0/g, '')

export const getGlobalConfigPda = (): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(GLOBAL_CONFIG_SEED)], PROGRAM_ID)

export const getBondingCurvePda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(BONDING_CURVE_SEED), mint.toBuffer()], PROGRAM_ID)

export const getTreasuryTokenAccount = (mint: PublicKey, treasury: PublicKey): PublicKey =>
  getAssociatedTokenAddressSync(mint, treasury, true, TOKEN_2022_PROGRAM_ID)

export const getUserPositionPda = (bondingCurve: PublicKey, user: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(USER_POSITION_SEED), bondingCurve.toBuffer(), user.toBuffer()],
    PROGRAM_ID,
  )

export const getTokenTreasuryPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(TREASURY_SEED), mint.toBuffer()], PROGRAM_ID)

export const getProtocolTreasuryPda = (): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(PROTOCOL_TREASURY_SEED)], PROGRAM_ID)

export const getUserStatsPda = (user: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(USER_STATS_SEED), user.toBuffer()], PROGRAM_ID)

export const getStarRecordPda = (user: PublicKey, mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(STAR_RECORD_SEED), user.toBuffer(), mint.toBuffer()],
    PROGRAM_ID,
  )

export const getLoanPositionPda = (mint: PublicKey, borrower: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(LOAN_SEED), mint.toBuffer(), borrower.toBuffer()],
    PROGRAM_ID,
  )

export const getCollateralVaultPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(COLLATERAL_VAULT_SEED), mint.toBuffer()],
    PROGRAM_ID,
  )

export const getTorchVaultPda = (creator: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(TORCH_VAULT_SEED), creator.toBuffer()], PROGRAM_ID)

// System-owned companion PDA — sol_source in the deep_pool vault_swap CPI.
export const getVaultSolPda = (creator: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(TORCH_VAULT_SOL_SEED), creator.toBuffer()],
    PROGRAM_ID,
  )

export const getVaultWalletLinkPda = (wallet: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(VAULT_WALLET_LINK_SEED), wallet.toBuffer()],
    PROGRAM_ID,
  )

export const getTreasuryLockPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(TREASURY_LOCK_SEED), mint.toBuffer()], PROGRAM_ID)

export const getShortPositionPda = (mint: PublicKey, shorter: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(SHORT_SEED), mint.toBuffer(), shorter.toBuffer()],
    PROGRAM_ID,
  )

export const getShortConfigPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from(SHORT_CONFIG_SEED), mint.toBuffer()], PROGRAM_ID)

export const getTreasuryLockTokenAccount = (
  mint: PublicKey,
  treasuryLock: PublicKey,
): PublicKey => {
  return getAssociatedTokenAddressSync(
    mint,
    treasuryLock,
    true, // allowOwnerOffCurve (PDA)
    TOKEN_2022_PROGRAM_ID,
  )
}

export const getProgram = (provider: AnchorProvider): Program =>
  new Program(idl as unknown, provider)

// tokens out for a given SOL amount
export const calculateTokensOut = (
  solAmount: bigint,
  virtualSolReserves: bigint,
  virtualTokenReserves: bigint,
  realSolReserves: bigint = BigInt(0), // V2.3: needed for dynamic rate calculation
  protocolFeeBps: number = 50, // [V4.0] 0.5% protocol fee (90% protocol treasury, 10% dev)
  treasuryFeeBps: number = 0, // [V10] 0% token treasury fee (removed — treasury funded by dynamic SOL rate + transfer fees)
  bondingTarget: bigint = BigInt('200000000000'), // [V24] per-token target (0 = 200 SOL)
): {
  tokensOut: bigint
  tokensToUser: bigint
  protocolFee: bigint
  treasuryFee: bigint
  solToCurve: bigint
  solToTreasury: bigint
  solToCreator: bigint // [V34] Creator SOL share
  treasuryRateBps: number // V2.3: the dynamic total rate used
  creatorRateBps: number // [V34] Creator rate used
} => {
  // calculate protocol fee (1%)
  const protocolFee = (solAmount * BigInt(protocolFeeBps)) / BigInt(10000)
  // calculate treasury fee (1%)
  const treasuryFee = (solAmount * BigInt(treasuryFeeBps)) / BigInt(10000)
  const solAfterFees = solAmount - protocolFee - treasuryFee
  // flat 20% → 5% treasury rate across all tiers
  const resolvedTarget = bondingTarget === BigInt(0) ? BigInt('200000000000') : bondingTarget
  // dynamic treasury rate - decays from 20% to 5% as bonding progresses
  const rateRange = BigInt(TREASURY_SOL_MAX_BPS - TREASURY_SOL_MIN_BPS)
  const decay = (realSolReserves * rateRange) / resolvedTarget
  const treasuryRateBps = Math.max(TREASURY_SOL_MAX_BPS - Number(decay), TREASURY_SOL_MIN_BPS)
  // creator rate - grows from 0.2% to 1% (inverse of treasury decay)
  const creatorRange = BigInt(CREATOR_SOL_MAX_BPS - CREATOR_SOL_MIN_BPS)
  const creatorGrowth = (realSolReserves * creatorRange) / resolvedTarget
  const creatorRateBps = Math.min(CREATOR_SOL_MIN_BPS + Number(creatorGrowth), CREATOR_SOL_MAX_BPS)
  // split remaining SOL: total rate → creator + treasury + curve
  const totalSplit = (solAfterFees * BigInt(treasuryRateBps)) / BigInt(10000)
  const solToCreator = (solAfterFees * BigInt(creatorRateBps)) / BigInt(10000)
  const solToTreasurySplit = totalSplit - solToCreator
  const solToCurve = solAfterFees - totalSplit
  // total to treasury = flat fee + dynamic split (minus creator)
  const solToTreasury = treasuryFee + solToTreasurySplit
  // constant product: tokens out for the SOL that enters the curve
  const tokensOut = (virtualTokenReserves * solToCurve) / (virtualSolReserves + solToCurve)
  const tokensToUser = tokensOut
  return {
    tokensOut,
    tokensToUser,
    protocolFee,
    treasuryFee,
    solToCurve,
    solToTreasury,
    solToCreator,
    treasuryRateBps,
    creatorRateBps,
  }
}

// calculate SOL out for a given token amount (no sell fee)
export const calculateSolOut = (
  tokenAmount: bigint,
  virtualSolReserves: bigint,
  virtualTokenReserves: bigint,
): { solOut: bigint; solToUser: bigint } => {
  // calculate SOL using inverse formula
  const solOut = (virtualSolReserves * tokenAmount) / (virtualTokenReserves + tokenAmount)
  // no fees on sells - user gets full amount
  return { solOut, solToUser: solOut }
}

// calculate current token price in SOL
export const calculatePrice = (
  virtualSolReserves: bigint,
  virtualTokenReserves: bigint,
): number => {
  // price = virtualSol / virtualTokens
  return Number(virtualSolReserves) / Number(virtualTokenReserves)
}

export const calculateBondingProgress = (realSolReserves: bigint): number => {
  const target = BigInt('200000000000') // 200 SOL in lamports
  if (realSolReserves >= target) {
    return 100
  }
  return (Number(realSolReserves) / Number(target)) * 100
}

// ============================================================================
// DEEPPOOL PDA DERIVATION
// ============================================================================
//
// re-exported from deeppoolsdk (the authoritative source) under torchsdk's historical names for backward compatibility.
// if deep_pool ever changes seed shape, the update lands in deeppoolsdk and torchsdk gets it for free.

import {
  getPoolPda as _getPoolPda,
  getVaultPda as _getVaultPda,
  getLpMintPda as _getLpMintPda,
  getEventAuthorityPda as _getEventAuthorityPda,
} from 'deeppoolsdk'

export const getDeepPoolPda = _getPoolPda
export const getDeepPoolVaultPda = _getVaultPda
export const getDeepPoolLpMintPda = _getLpMintPda
// deep_pool's emit_cpi! authority — required by every deep_pool ix as of v4.x
export const getDeepPoolEventAuthorityPda = _getEventAuthorityPda

// torch config PDA — used as signer-verified namespace for DeepPool pools
export const getTorchConfigPda = (): [PublicKey, number] =>
  PublicKey.findProgramAddressSync([Buffer.from('torch_config')], PROGRAM_ID)

// get all DeepPool accounts for a Torch token (uses torch config namespace)
export const getDeepPoolAccounts = (
  tokenMint: PublicKey,
): {
  pool: PublicKey
  tokenVault: PublicKey
  lpMint: PublicKey
  config: PublicKey
  eventAuthority: PublicKey
} => {
  const [config] = getTorchConfigPda()
  const [pool] = getDeepPoolPda(config, tokenMint)
  const [tokenVault] = getDeepPoolVaultPda(pool)
  const [lpMint] = getDeepPoolLpMintPda(pool)
  const [eventAuthority] = getDeepPoolEventAuthorityPda()
  return { pool, tokenVault, lpMint, config, eventAuthority }
}
