import { Program, AnchorProvider, BN } from '@coral-xyz/anchor'
import { PublicKey } from '@solana/web3.js'
import {
  PROGRAM_ID,
  GLOBAL_CONFIG_SEED,
  BONDING_CURVE_SEED,
  BONDING_CURVE_SOL_SEED,
  TREASURY_SEED,
  TREASURY_SOL_VAULT_SEED,
  USER_POSITION_SEED,
  PROTOCOL_TREASURY_SEED,
  USER_STATS_SEED,
  TORCH_VAULT_SEED,
  TORCH_VAULT_SOL_SEED,
  VAULT_WALLET_LINK_SEED,
  TREASURY_LOCK_SEED,
  POSITION_SEED,
  SHORT_VAULT_SEED,
  LONG_SOL_VAULT_SEED,
  POSITION_SIDE_LONG,
  POSITION_SIDE_SHORT,
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
  total_tokens_launched: BN
  total_volume_sol: BN
  bump: number
}

// [V21] Treasury is accounting-only — lendable SOL lives in the System-owned
// treasury_sol_vault PDA (see TREASURY_SOL_VAULT_SEED). Star/creator-reward and
// the V20 single loan/short counters were removed; long+short are tracked
// separately.
export interface Treasury {
  bonding_curve: PublicKey
  mint: PublicKey
  is_community_token: boolean
  last_buyback_slot: BN
  harvested_fees: BN
  bump: number
  baseline_sol_reserves: BN
  baseline_token_reserves: BN
  short_selling_enabled: boolean
  min_buyback_interval_slots: BN
  baseline_initialized: boolean
  total_tokens_lent: BN
  active_shorts: BN
  short_interest_collected: BN
  total_sol_lent_to_longs: BN
  active_longs: BN
  long_interest_collected: BN
  total_token_collateral_locked: BN
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

// [V21] Unified leverage position. One struct for both sides; a user can hold
// many per (mint, side) distinguished by `position_index`. Unit-by-side:
//   short → collateral_amount = lamports (SOL), debt_amount = tokens
//   long  → collateral_amount = tokens,          debt_amount = lamports (SOL)
export type PositionSide = { long: Record<string, never> } | { short: Record<string, never> }

export interface Position {
  user: PublicKey
  mint: PublicKey
  side: PositionSide
  position_index: number
  collateral_amount: BN
  debt_amount: BN
  accrued_interest: BN
  last_slot: BN
  bump: number
  vault_bump: number
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

// [V21] System-owned per-mint vault custodying lendable SOL.
export const getTreasurySolVaultPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(TREASURY_SOL_VAULT_SEED), mint.toBuffer()],
    PROGRAM_ID,
  )

// [V21] System-owned vault holding the bonding curve's real SOL reserves.
export const getBondingCurveSolPda = (mint: PublicKey): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(BONDING_CURVE_SOL_SEED), mint.toBuffer()],
    PROGRAM_ID,
  )

// little-endian u32 — matches `args.position_index.to_le_bytes()` in the program.
const u32le = (n: number): Buffer => {
  const b = Buffer.alloc(4)
  b.writeUInt32LE(n >>> 0, 0)
  return b
}

// [V21] Unified leverage position PDA. Seeds:
//   [POSITION_SEED, owner, mint, [sideByte], position_index_le]
// owner is the wallet (direct) or the TorchVault PDA (via_vault).
export const getPositionPda = (
  owner: PublicKey,
  mint: PublicKey,
  side: 'long' | 'short',
  positionIndex: number,
): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [
      Buffer.from(POSITION_SEED),
      owner.toBuffer(),
      mint.toBuffer(),
      Buffer.from([side === 'short' ? POSITION_SIDE_SHORT : POSITION_SIDE_LONG]),
      u32le(positionIndex),
    ],
    PROGRAM_ID,
  )

// [V21] System-owned SOL vault for a SHORT position (collateral custody).
export const getShortVaultPda = (
  owner: PublicKey,
  mint: PublicKey,
  positionIndex: number,
): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(SHORT_VAULT_SEED), owner.toBuffer(), mint.toBuffer(), u32le(positionIndex)],
    PROGRAM_ID,
  )

// [V21] System-owned SOL vault for a LONG position (borrowed-SOL custody).
export const getLongSolVaultPda = (
  owner: PublicKey,
  mint: PublicKey,
  positionIndex: number,
): [PublicKey, number] =>
  PublicKey.findProgramAddressSync(
    [Buffer.from(LONG_SOL_VAULT_SEED), owner.toBuffer(), mint.toBuffer(), u32le(positionIndex)],
    PROGRAM_ID,
  )

// [V21] A LONG position's token collateral vault — an ATA owned by the position PDA.
export const getPositionTokenVault = (mint: PublicKey, position: PublicKey): PublicKey =>
  getAssociatedTokenAddressSync(mint, position, true, TOKEN_2022_PROGRAM_ID)

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

export const calculateBondingProgress = (
  realSolReserves: bigint,
  bondingTarget?: bigint,
): number => {
  const target =
    bondingTarget !== undefined && bondingTarget > BigInt(0)
      ? bondingTarget
      : BigInt('200000000000')
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

// ============================================================================
// [V21] Depth-scaled risk rails — mirror of programs/torch_market/src/{constants,
// pool_validation}.rs. Depth is priced once at open by these pure functions; see
// docs/depth-scaled-risk-rails.md.
// ============================================================================

/** 100 SOL (lamports) — smallest pool we lever; below it, no leverage. */
export const DEPTH_FLOOR_SOL = 100_000_000_000
/** Max LTV at the floor (30%) and the asymptote (60%). */
export const LTV_MIN_BPS = 3000
export const LTV_MAX_BPS = 6000
/** Size cap: a position's SOL-debt-value ≤ ρ_max of pool SOL (25%). */
export const RHO_MAX_BPS = 2500

/**
 * Rail 1 — continuous concave max LTV in bps from pool SOL (lamports):
 * `LTV_MAX − (LTV_MAX − LTV_MIN)·S_floor/x`, clamped to [LTV_MIN, LTV_MAX];
 * 0 below the floor. 30% at 100 SOL → 60% asymptote. Mirror of the Rust fn.
 */
export const getDepthMaxLtvBps = (poolSol: number): number => {
  if (poolSol < DEPTH_FLOOR_SOL) return 0
  const span = LTV_MAX_BPS - LTV_MIN_BPS // 3000
  const drop = Math.floor((span * DEPTH_FLOOR_SOL) / poolSol)
  return Math.min(LTV_MAX_BPS, Math.max(LTV_MIN_BPS, LTV_MAX_BPS - drop))
}

/**
 * Rail 2 — size cap: the maximum SOL-debt-value a single position may carry,
 * = ρ_max of pool SOL (lamports). The program clamps the borrow to this at open.
 * (Divides before multiplying to stay inside JS safe-integer range on deep pools.)
 */
export const maxDebtValueForDepth = (poolSol: number): number =>
  Math.floor((poolSol / 10000) * RHO_MAX_BPS)
