import { PublicKey } from '@solana/web3.js'
import {
  PROGRAM_ID as SDK_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID as SDK_TOKEN_2022_PROGRAM_ID,
  BONDING_CURVE_SEED as SDK_BONDING_CURVE_SEED,
  TREASURY_SEED as SDK_TREASURY_SEED,
  USER_POSITION_SEED as SDK_USER_POSITION_SEED,
  PROTOCOL_TREASURY_SEED as SDK_PROTOCOL_TREASURY_SEED,
  USER_STATS_SEED as SDK_USER_STATS_SEED,
  TREASURY_LOCK_SEED as SDK_TREASURY_LOCK_SEED,
  TOTAL_SUPPLY as SDK_TOTAL_SUPPLY,
  TOKEN_DECIMALS as SDK_TOKEN_DECIMALS,
  LAMPORTS_PER_SOL as SDK_LAMPORTS_PER_SOL,
  TOKEN_MULTIPLIER as SDK_TOKEN_MULTIPLIER,
} from 'torchsdk'

// Re-export the SDK's program/seed/constants under the app's existing names
// so callsites keep working. Single source of truth is torchsdk.
export const PROGRAM_ID = SDK_PROGRAM_ID
export const TOKEN_2022_PROGRAM_ID = SDK_TOKEN_2022_PROGRAM_ID
export const BONDING_CURVE_SEED = SDK_BONDING_CURVE_SEED
export const TREASURY_SEED = SDK_TREASURY_SEED
export const USER_POSITION_SEED = SDK_USER_POSITION_SEED
export const PROTOCOL_TREASURY_SEED = SDK_PROTOCOL_TREASURY_SEED
export const USER_STATS_SEED = SDK_USER_STATS_SEED
export const TREASURY_LOCK_SEED = SDK_TREASURY_LOCK_SEED
export const TOTAL_SUPPLY = SDK_TOTAL_SUPPLY
export const TOKEN_DECIMALS = SDK_TOKEN_DECIMALS
export const LAMPORTS_PER_SOL = SDK_LAMPORTS_PER_SOL
export const TOKEN_MULTIPLIER = SDK_TOKEN_MULTIPLIER

// Simnet program ID — for local Surfpool testing with fresh deploys
export const SIMNET_PROGRAM_ID = new PublicKey('D7bgRJZgn3tsfHY3bkgSCTRZRwaMYM8wxdmLmzM9yWkh')

// WSOL Mint (kept for ATA derivation in wrapped-SOL flows)
export const WSOL_MINT = new PublicKey('So11111111111111111111111111111111111111112')

// Token constants (must match the Rust program)
export const MAX_WALLET_TOKENS = BigInt('20000000000000') // 2% of supply
export const BONDING_TARGET_LAMPORTS = BigInt('200000000000') // 200 SOL

// V23: Tiered Bonding Curves
export const BONDING_TARGET_SPARK = BigInt('50000000000')   // 50 SOL
export const BONDING_TARGET_FLAME = BigInt('100000000000')  // 100 SOL
export const BONDING_TARGET_TORCH = BigInt('200000000000')  // 200 SOL (default)
// [V4.0] Spark removed from creation — existing tokens still function
export const VALID_BONDING_TARGETS = [
  BONDING_TARGET_FLAME,
  BONDING_TARGET_TORCH,
] as const

export const INITIAL_VIRTUAL_SOL = BigInt('30000000000') // 30 SOL
export const INITIAL_VIRTUAL_TOKENS = BigInt('107300000000000') // Scaled for 1B supply

// V31: Zero-Burn Distribution
// 300M locked in treasury PDA, 700M for curve + pool
// IVS = 3BT/8, IVT = 756.25M — 13.44x multiplier (pump.fun-aligned)
export const TREASURY_LOCK_TOKENS = BigInt('300000000000000')       // 300M locked (30%)
export const CURVE_SUPPLY = BigInt('700000000000000')                // 700M for curve + pool (70%)
export const INITIAL_VIRTUAL_TOKENS_V27 = BigInt('756250000000000') // 756.25M tokens

/** [V27] Returns virtual reserves for a given bonding tier. IVS = 3BT/8, 13.44x multiplier. */
export function initialVirtualReserves(bondingTarget: bigint): { ivs: bigint; ivt: bigint } {
  const bt = bondingTarget.toString()
  if (bt === '50000000000')  return { ivs: BigInt('18750000000'), ivt: INITIAL_VIRTUAL_TOKENS_V27 }
  if (bt === '100000000000') return { ivs: BigInt('37500000000'), ivt: INITIAL_VIRTUAL_TOKENS_V27 }
  if (bt === '200000000000') return { ivs: BigInt('75000000000'), ivt: INITIAL_VIRTUAL_TOKENS_V27 }
  return { ivs: INITIAL_VIRTUAL_SOL, ivt: INITIAL_VIRTUAL_TOKENS }
}

export const MIN_SOL_AMOUNT = BigInt('1000000') // 0.001 SOL

// Failed token reclaim and platform rewards
export const INACTIVITY_PERIOD_SLOTS = BigInt((7 * 24 * 60 * 60 * 1000) / 400) // ~7 days in slots
export const EPOCH_DURATION_SECONDS = 7 * 24 * 60 * 60 // 7 days

// Protocol Treasury — minimum prior-epoch volume to be eligible for claims
export const MIN_EPOCH_VOLUME_ELIGIBILITY = BigInt('2000000000') // 2 SOL

// Formatting helpers
export function formatSol(lamports: bigint | number): string {
  const value = Number(lamports) / LAMPORTS_PER_SOL
  return value.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  })
}

// Format a SOL-denominated number (already converted from lamports).
// Use with values returned from torchsdk readers (e.g., ProtocolTreasuryInfo.current_balance_sol).
export function formatSolAmount(sol: number): string {
  return sol.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  })
}

export function formatTokens(amount: bigint | number): string {
  const value = Number(amount) / TOKEN_MULTIPLIER
  if (value >= 1_000_000_000) {
    return (value / 1_000_000_000).toFixed(2) + 'B'
  }
  if (value >= 1_000_000) {
    return (value / 1_000_000).toFixed(2) + 'M'
  }
  if (value >= 1_000) {
    return (value / 1_000).toFixed(2) + 'K'
  }
  return value.toFixed(2)
}

export function formatPercent(value: number): string {
  return (value * 100).toFixed(2) + '%'
}

export function shortenAddress(address: string, chars = 4): string {
  return `${address.slice(0, chars)}...${address.slice(-chars)}`
}

// Estimate time from slot difference (Solana ~400ms per slot)
const MS_PER_SLOT = 400

export function formatSlotAge(slot: bigint, currentSlot: bigint): string {
  const slotDiff = currentSlot - slot
  if (slotDiff <= BigInt(0)) return 'Just now'

  const msAgo = Number(slotDiff) * MS_PER_SLOT
  const seconds = Math.floor(msAgo / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)
  const days = Math.floor(hours / 24)

  if (days > 0) return `${days}d ago`
  if (hours > 0) return `${hours}h ago`
  if (minutes > 0) return `${minutes}m ago`
  return 'Just now'
}
