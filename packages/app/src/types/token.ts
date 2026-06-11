/**
 * Token Types
 *
 * Shared type definitions for token-related data structures.
 * Uses torchsdk types as the source of truth.
 */

import type { TokenSummary as SdkTokenSummary } from 'torchsdk'
import { LEGACY_MINTS } from 'torchsdk'

/** Token tier based on bonding curve SOL target */
export type TokenTier = 'flame' | 'torch'

/** Tier display metadata */
export const TIER_CONFIG: Record<TokenTier, { label: string; solTarget: number; color: string; description: string }> = {
  flame: { label: 'Flame', solTarget: 100, color: '#EF4444', description: '13.44x · 100 SOL' },
  torch: { label: 'Torch', solTarget: 200, color: '#F97316', description: '13.44x · 200 SOL' },
}

/** SOL target in lamports for each tier */
export const SOL_TARGET_MAP: Record<TokenTier, number> = {
  flame: 100_000_000_000,
  torch: 200_000_000_000,
}

/** Derive tier from bonding target in lamports. 0 = pre-v3.3.0, default to torch (200 SOL). */
export function getTierFromTarget(bondingTarget: number): TokenTier {
  // Spark (50 SOL) removed from the program — legacy 50-SOL devnet tokens
  // render as flame (≤ 100 SOL bucket), matching the indexer's bucketing.
  if (!bondingTarget || bondingTarget <= 0) return 'torch'
  if (bondingTarget <= 100_000_000_000) return 'flame'
  return 'torch'
}

/** Enrichment fields from TokenDetail (not on TokenSummary) */
export interface TokenEnrichment {
  image?: string
  creator?: string
  creator_verified?: boolean
  creator_trust_tier?: 'high' | 'medium' | 'low' | null
  creator_said_name?: string
  creator_badge_url?: string
  last_activity_at?: number
  bonding_target?: number
  tier?: TokenTier
  // Corrected values (override SDK defaults that assume 200 SOL / bonding curve price)
  progress_percent?: number
  price_sol?: number
  market_cap_sol?: number
}

/** Extended token data: SDK summary + optional enrichment from TokenDetail */
export type TokenData = SdkTokenSummary & TokenEnrichment

/** Filter options for token list */
export type TokenFilter = 'bonding' | 'complete' | 'reclaimed' | 'legacy'

/** Filter labels for display */
export const FILTER_LABELS: Record<TokenFilter, string> = {
  bonding: 'Bonding',
  complete: 'Active',
  reclaimed: 'Reclaimed',
  legacy: 'Legacy',
}

/** Token status derived from SDK status + progress */
export type TokenStatus = 'new' | 'bonding' | 'complete' | 'migrated' | 'reclaimed' | 'legacy'

/** Set of legacy mint addresses for O(1) lookup */
const LEGACY_SET = new Set(LEGACY_MINTS)

/** Check if a token mint is a legacy token */
export function isLegacyMint(mint: string): boolean {
  return LEGACY_SET.has(mint)
}

/**
 * Get the display status of a token from its SDK data
 */
export function getTokenStatus(token: TokenData): TokenStatus {
  if (isLegacyMint(token.mint)) return 'legacy'
  // SDK status is 'bonding' | 'complete' | 'migrated' | 'reclaimed'
  if (token.status === 'reclaimed') return 'reclaimed'
  if (token.status === 'migrated') return 'migrated'
  if (token.status === 'complete') return 'complete'
  // For bonding tokens, distinguish 'new' (< 25 SOL raised = < 12.5% progress)
  if (token.progress_percent < 12.5) return 'new'
  return 'bonding'
}

/**
 * Check if a token matches a given filter
 */
export function matchesFilter(token: TokenData, filter: TokenFilter): boolean {
  const status = getTokenStatus(token)

  // Legacy tokens only appear in the legacy tab
  if (status === 'legacy') return filter === 'legacy'

  switch (filter) {
    case 'bonding':
      return status === 'new' || status === 'bonding'
    case 'complete':
      return token.status === 'complete' || token.status === 'migrated'
    case 'reclaimed':
      return token.status === 'reclaimed'
    case 'legacy':
      return false // handled above
    default:
      return false
  }
}
