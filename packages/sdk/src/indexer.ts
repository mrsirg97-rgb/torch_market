// Indexer HTTP client.
//
// Reads through the torch-indexer's API (typically running locally —
// `docker compose --profile full up` from packages/.../indexer). Used by
// hot-path getters in `tokens.ts` as the fast path; falls back to RPC
// silently on any failure.
//
// Two categories of methods here:
//   1. Internal helpers (`fetchLoansFromIndexer`, `fetchShortsFromIndexer`,
//      `fetchMessagesFromIndexer`) consumed by tokens.ts to populate the
//      indexer-first path with RPC fallback.
//   2. Public indexer-only methods (`getTrades`, `getCandles`) that have no
//      RPC equivalent — chains can't aggregate historical events in any
//      reasonable time. These throw if the indexer is unreachable.
//
// Wire types in this file mirror indexer/src/contracts.rs row shapes exactly
// (serde uses snake_case for fields and `rename_all` for enum variants —
// e.g. MarketStatus::Rs → "RS").

// ============================================================================
// Indexer wire types — direct mirror of contracts.rs
// ============================================================================

export type IndexerMarketStatus = 'RS' | 'RD' | 'ASN' | 'MIGRATED' | 'RECLAIMED'
export type IndexerMarketTier = 'spark' | 'flame' | 'torch'
export type IndexerPositionHealth = 'healthy' | 'at_risk' | 'liquidatable' | 'none'

export interface IndexerMarketRow {
  mint: string
  name: string
  symbol: string
  metadata_uri: string | null
  image_url: string | null
  creator: string
  is_community_token: boolean
  status: IndexerMarketStatus
  tier: IndexerMarketTier
  sol_target: number
  virtual_sol: number
  virtual_token: number
  real_sol: number
  real_token: number
  created_at_slot: number
  bonding_complete_slot: number | null
  migrated_slot: number | null
  reclaimed_slot: number | null
  last_activity_slot: number
  deep_pool_pubkey: string | null
  created_at: string // ISO timestamp
  updated_at: string
}

export interface IndexerTradeRow {
  trade_id: number
  mint: string
  trader: string
  vault: string | null
  is_buy: boolean
  sol_in: number
  sol_out: number
  tokens_in: number
  tokens_out: number
  sol_to_treasury: number
  sol_to_creator: number
  protocol_fee: number
  virtual_sol_after: number
  virtual_token_after: number
  real_sol_after: number
  real_token_after: number
  slot: number
  signature: string
  inner_ix_idx: number
  created_at: string
}

export interface IndexerMessageRow {
  message_id: number
  mint: string
  sender: string
  memo_text: string
  action_kind: string | null
  slot: number
  signature: string
  inner_ix_idx: number
  created_at: string
}

export interface IndexerLoanRow {
  mint: string
  borrower: string
  collateral_amount: number
  borrowed_amount: number
  accrued_interest_stored: number
  last_update_slot: number
  health: IndexerPositionHealth
  is_active: boolean
  created_at: string
  updated_at: string
}

export interface IndexerShortRow {
  mint: string
  shorter: string
  sol_collateral: number
  tokens_borrowed: number
  accrued_interest_stored: number
  last_update_slot: number
  health: IndexerPositionHealth
  is_active: boolean
  created_at: string
  updated_at: string
}

export interface IndexerCandle {
  bucket_start: string
  open: number | null
  high: number | null
  low: number | null
  close: number | null
  volume: number | null
}

// ============================================================================
// Public query shapes for indexer-only readers
// ============================================================================

export interface TradeHistoryQuery {
  indexer: string
  mint?: string
  trader?: string
  isBuy?: boolean
  since?: Date
  before?: Date
  limit?: number
}

export interface CandlesQuery {
  indexer: string
  mint: string
  interval: '1m' | '5m' | '1h'
  since?: Date
  before?: Date
}

// ============================================================================
// Internal: HTTP + fallback plumbing
// ============================================================================

async function indexerFetch<T>(indexer: string, path: string): Promise<T> {
  const resp = await fetch(`${indexer}${path}`)
  if (!resp.ok) {
    throw new Error(`indexer ${path}: HTTP ${resp.status}`)
  }
  return resp.json() as Promise<T>
}

/**
 * Generic indexer-first wrapper. Try `primary`; on any throw, run `fallback`.
 * Used by tokens.ts to make individual getters opt-in to acceleration
 * without losing functionality when the indexer is unavailable.
 */
export async function withFallback<T>(
  primary: () => Promise<T>,
  fallback: () => Promise<T>,
): Promise<T> {
  try {
    return await primary()
  } catch {
    return fallback()
  }
}

// ============================================================================
// Internal helpers — indexer-first paths called from tokens.ts
// ============================================================================

export async function fetchLoansFromIndexer(
  indexer: string,
  mint: string,
  isActive: boolean | null = true,
): Promise<IndexerLoanRow[]> {
  const params = new URLSearchParams()
  params.set('mint', mint)
  if (isActive !== null) params.set('is_active', String(isActive))
  // The active-loans UI rarely needs more than a few hundred rows; cap to
  // the indexer's own clamp (500) so we don't fetch a giant payload.
  params.set('limit', '500')
  return indexerFetch<IndexerLoanRow[]>(indexer, `/api/loans?${params.toString()}`)
}

export async function fetchShortsFromIndexer(
  indexer: string,
  mint: string,
  isActive: boolean | null = true,
): Promise<IndexerShortRow[]> {
  const params = new URLSearchParams()
  params.set('mint', mint)
  if (isActive !== null) params.set('is_active', String(isActive))
  params.set('limit', '500')
  return indexerFetch<IndexerShortRow[]>(indexer, `/api/shorts?${params.toString()}`)
}

export async function fetchMessagesFromIndexer(
  indexer: string,
  mint: string,
  limit = 50,
): Promise<IndexerMessageRow[]> {
  const params = new URLSearchParams()
  params.set('mint', mint)
  params.set('limit', String(limit))
  return indexerFetch<IndexerMessageRow[]>(indexer, `/api/messages?${params.toString()}`)
}

export async function fetchMarketFromIndexer(
  indexer: string,
  mint: string,
): Promise<IndexerMarketRow | null> {
  try {
    const detail = await indexerFetch<{
      market: IndexerMarketRow
      reserves: unknown
    }>(indexer, `/api/markets/${mint}`)
    return detail.market
  } catch (e) {
    if ((e as Error).message?.includes('HTTP 404')) return null
    throw e
  }
}

export interface MarketsFilter {
  status?: IndexerMarketStatus
  tier?: IndexerMarketTier
  creator?: string
  since?: Date
  before?: Date
  limit?: number
}

export async function fetchMarketsFromIndexer(
  indexer: string,
  filter: MarketsFilter = {},
): Promise<IndexerMarketRow[]> {
  const params = new URLSearchParams()
  if (filter.status) params.set('status', filter.status)
  if (filter.tier) params.set('tier', filter.tier)
  if (filter.creator) params.set('creator', filter.creator)
  if (filter.since) params.set('since', filter.since.toISOString())
  if (filter.before) params.set('before', filter.before.toISOString())
  // Indexer clamps to 500 anyway; ask for it explicitly so a no-filter
  // list view gets the full page in one request.
  params.set('limit', String(filter.limit ?? 500))
  return indexerFetch<IndexerMarketRow[]>(indexer, `/api/markets?${params.toString()}`)
}

// ============================================================================
// Indexer-only public readers
// ============================================================================

/**
 * Trade history for a mint, spanning both the bonding-curve (`trades`
 * table) and post-migration DEX (`swaps` table — populated when this
 * indexer also ingests deep_pool events). Returns raw trade rows ordered
 * newest-first by slot.
 *
 * Indexer-only — RPC has no efficient way to aggregate this.
 */
export async function getTrades(query: TradeHistoryQuery): Promise<IndexerTradeRow[]> {
  const params = new URLSearchParams()
  if (query.mint) params.set('mint', query.mint)
  if (query.trader) params.set('trader', query.trader)
  if (query.isBuy != null) params.set('is_buy', String(query.isBuy))
  if (query.since) params.set('since', query.since.toISOString())
  if (query.before) params.set('before', query.before.toISOString())
  if (query.limit != null) params.set('limit', String(query.limit))
  const qs = params.toString()
  return indexerFetch<IndexerTradeRow[]>(
    query.indexer,
    qs ? `/api/trades?${qs}` : '/api/trades',
  )
}

/**
 * OHLCV candles for a mint at the requested interval. Buckets are computed
 * server-side via `floor(epoch / interval_seconds) * interval_seconds`,
 * which works in vanilla Postgres (no TimescaleDB needed). The indexer
 * unions bonding-curve trades and post-migration DEX swaps so the chart
 * spans the full lifecycle.
 *
 * Indexer-only.
 */
export async function getCandles(query: CandlesQuery): Promise<IndexerCandle[]> {
  const params = new URLSearchParams()
  params.set('mint', query.mint)
  params.set('interval', query.interval)
  if (query.since) params.set('since', query.since.toISOString())
  if (query.before) params.set('before', query.before.toISOString())
  return indexerFetch<IndexerCandle[]>(
    query.indexer,
    `/api/candles?${params.toString()}`,
  )
}
