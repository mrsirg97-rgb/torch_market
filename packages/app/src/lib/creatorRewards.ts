/**
 * Star Token System (V10)
 *
 * Interfaces and helpers for the star token system.
 * Users can star tokens they appreciate (0.02 SOL per star).
 * At 2000 stars, accumulated SOL is auto-paid to the creator.
 */

import { PublicKey } from '@solana/web3.js'
import { BN } from '@coral-xyz/anchor'
import { PROGRAM_ID } from './constants'

// ============================================================================
// CONSTANTS
// ============================================================================

export const STAR_RECORD_SEED = 'star_record'

/** Stars required for creator auto-payout */
export const STAR_THRESHOLD = 2000

/** Cost per star in lamports (0.02 SOL) */
export const STAR_COST_LAMPORTS = BigInt('20000000')

// ============================================================================
// INTERFACES (matches Anchor on-chain accounts)
// ============================================================================

/**
 * Individual star record (prevents double-starring)
 *
 * PDA seeds: [STAR_RECORD_SEED, user, mint]
 *
 * V10 Change: Stars are now per-token (mint), not per-creator
 */
export interface StarRecord {
  /** User who gave the star */
  user: PublicKey
  /** Token mint that received the star */
  mint: PublicKey
  /** Slot when star was given */
  starred_at_slot: BN
  /** PDA bump */
  bump: number
}

/**
 * Star tracking fields on Treasury account (V10)
 *
 * These fields are part of the Treasury account, not a separate account.
 */
export interface TreasuryStarFields {
  /** Total stars received on this token */
  total_stars: BN
  /** Accumulated star SOL (0.05 SOL per star) */
  star_sol_balance: BN
  /** True after creator received auto-payout at 2000 stars */
  creator_paid_out: boolean
}

// ============================================================================
// PDA DERIVATION
// ============================================================================

/**
 * Derive StarRecord PDA for a user-token pair (V10)
 *
 * @param user - User who starred
 * @param mint - Token mint that was starred
 */
export function getStarRecordPda(user: PublicKey, mint: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from(STAR_RECORD_SEED), user.toBuffer(), mint.toBuffer()],
    PROGRAM_ID,
  )
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/**
 * Check if creator has received payout (2000+ stars)
 */
export function hasCreatorBeenPaid(
  totalStars: BN | bigint | number,
  creatorPaidOut: boolean,
): boolean {
  return creatorPaidOut
}

/**
 * Check if token has reached payout threshold
 */
export function hasReachedPayoutThreshold(totalStars: BN | bigint | number): boolean {
  const stars = typeof totalStars === 'number' ? totalStars : Number(totalStars)
  return stars >= STAR_THRESHOLD
}

/**
 * Calculate progress towards payout threshold (0-100%)
 */
export function calculateStarProgress(totalStars: BN | bigint | number): number {
  const stars = typeof totalStars === 'number' ? totalStars : Number(totalStars)
  if (stars >= STAR_THRESHOLD) return 100
  return (stars / STAR_THRESHOLD) * 100
}

/**
 * Format star count for display
 */
export function formatStarCount(totalStars: BN | bigint | number): string {
  const stars = typeof totalStars === 'number' ? totalStars : Number(totalStars)
  if (stars >= 1_000_000) {
    return (stars / 1_000_000).toFixed(1) + 'M'
  }
  if (stars >= 1_000) {
    return (stars / 1_000).toFixed(1) + 'K'
  }
  return stars.toString()
}

/**
 * Calculate potential payout at threshold
 */
export function calculatePotentialPayout(totalStars: BN | bigint | number): bigint {
  const stars = typeof totalStars === 'number' ? BigInt(totalStars) : BigInt(totalStars.toString())
  // Max payout is at threshold (additional stars don't increase payout)
  const effectiveStars = stars > BigInt(STAR_THRESHOLD) ? BigInt(STAR_THRESHOLD) : stars
  return effectiveStars * STAR_COST_LAMPORTS
}

/**
 * Format SOL amount for display
 */
export function formatStarSol(lamports: BN | bigint | number): string {
  const amount = typeof lamports === 'number' ? lamports : Number(lamports)
  const sol = amount / 1_000_000_000
  return sol.toFixed(2) + ' SOL'
}
