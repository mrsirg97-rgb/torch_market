// Indexer HTTP client.
//
// Reads through the torch-indexer's API (typically running locally —
// `docker compose --profile full up` from packages/.../indexer). Used by
// hot-path getters in `tokens.ts` as the fast path; falls back to RPC
// silently on any failure.
//
// Two categories of methods here:
//   1. Internal helpers (`fetchPositionsFromIndexer`,
//      `fetchMessagesFromIndexer`) consumed by tokens.ts to populate the
//      indexer-first path with RPC fallback.
//   2. Public indexer-only methods (`getTrades`, `getCandles`,
//      `getLiquidations`) that have no
//      RPC equivalent — chains can't aggregate historical events in any
//      reasonable time. These throw if the indexer is unreachable.
//
// Wire types in this file mirror indexer/src/contracts.rs row shapes exactly
// (serde uses snake_case for fields and `rename_all` for enum variants —
// e.g. MarketStatus::Bonding → "BONDING"). Relabeled 2026-06-10 (RS/RD/ASN dead).

// ============================================================================
// Indexer wire types — direct mirror of contracts.rs
// ============================================================================

export type IndexerMarketStatus = 'BONDING' | 'COMPLETE' | 'MIGRATED' | 'RECLAIMED'
export type IndexerMarketTier = 'flame' | 'torch' // spark removed from the program
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

export type IndexerPositionSide = 'long' | 'short'
export type IndexerPositionEventKind = 'open' | 'close' | 'liquidate'

// [V21] Unified leverage position (replaces IndexerLoanRow + IndexerShortRow).
// Unit-by-side: short → collateral=SOL/debt=tokens; long → collateral=tokens/debt=SOL.
export interface IndexerPositionRow {
  mint: string
  owner: string
  side: IndexerPositionSide
  position_index: number
  collateral_amount: number
  debt_amount: number
  open_fee_sol: number
  vault_balance: number
  accrued_interest_stored: number
  last_update_slot: number
  health: IndexerPositionHealth
  is_active: boolean
  owner_is_vault: boolean
  created_at: string
  updated_at: string
}

// [V21] Append-only leverage event log row. Kind-specific columns are null for
// non-applicable kinds (e.g. liquidation analytics are null on open/close).
export interface IndexerPositionEventRow {
  mint: string
  owner: string
  side: IndexerPositionSide
  position_index: number
  kind: IndexerPositionEventKind
  liquidator: string | null
  sol_in: number | null
  sol_out: number | null
  tokens_in: number | null
  tokens_out: number | null
  interest_paid: number | null
  principal_paid: number | null
  surplus_sol: number | null
  bad_debt: number | null
  twap_ltv: number | null
  bonus_bps: number | null
  seized: number | null
  residual: number | null
  fully_resolved: boolean | null
  slot: number
  signature: string
  inner_ix_idx: number
  created_at: string
}

/** Post-migration DEX swap row (deep_pool `swaps` table). One row per
 *  `SwapExecuted` event emitted by the deep_pool program. The indexer
 *  resolves pool_id → token_mint via the cached pools cache. */
export interface IndexerSwapRow {
  swap_id: number
  pool_id: number
  user_pk: string
  sol_source: string
  is_buy: boolean
  amount_in_gross: number
  amount_in_net: number
  amount_out_gross: number
  amount_out_net: number
  fee: number
  sol_reserve_after: number
  token_reserve_after: number
  slot: number
  signature: string
  inner_ix_idx: number
  created_at: string
}

export interface IndexerCandle {
  bucket_start: string
  open: number | null
  high: number | null
  low: number | null
  close: number | null
  volume: number | null
}

export interface UserPnlByMint {
  mint: string
  tokens_remaining: number
  cost_basis_remaining: number // lamports spent on remaining tokens
  realized_pnl: number // lamports — signed; can be negative
  total_buy_volume: number // lamports
  total_sell_volume: number // lamports
  trade_count: number
}

export interface UserPnlSummary {
  wallet: string
  by_mint: UserPnlByMint[]
  total_realized_pnl: number
  total_volume: number
  total_trade_count: number
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
  interval: '1s' | '15s' | '30s' | '1m' | '5m' | '15m' | '1h' | '4h'
  since?: Date
  before?: Date
}

export interface SwapsQuery {
  indexer: string
  /** Filter to a single token's pool. The indexer resolves pool_id from this. */
  tokenMint?: string
  poolId?: number
  user?: string
  since?: Date
  before?: Date
  limit?: number
}

// [V21] Query for the append-only leverage event log (/api/liquidations). Pass
// `kind` to widen beyond liquidations (the endpoint defaults to `liquidate`).
export interface LiquidationsQuery {
  indexer: string
  mint?: string
  owner?: string
  side?: IndexerPositionSide
  kind?: IndexerPositionEventKind
  limit?: number
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

// [V21] Unified position fetch (replaces fetchLoansFromIndexer +
// fetchShortsFromIndexer). Pass `side` to scope to long or short; omit for both.
export async function fetchPositionsFromIndexer(
  indexer: string,
  mint: string,
  side: IndexerPositionSide | null = null,
  isActive: boolean | null = true,
): Promise<IndexerPositionRow[]> {
  const params = new URLSearchParams()
  params.set('mint', mint)
  if (side !== null) params.set('side', side)
  if (isActive !== null) params.set('is_active', String(isActive))
  // The active-positions UI rarely needs more than a few hundred rows; cap to
  // the indexer's own clamp (500) so we don't fetch a giant payload.
  params.set('limit', '500')
  return indexerFetch<IndexerPositionRow[]>(indexer, `/api/positions?${params.toString()}`)
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
  return indexerFetch<IndexerTradeRow[]>(query.indexer, qs ? `/api/trades?${qs}` : '/api/trades')
}

// [V21] Indexer-only: the leverage event log. Defaults to liquidations (the
// `bad_debt`/`twap_ltv`/`bonus_bps`/`seized` analytics surface); pass `kind` to
// widen. No RPC equivalent — chains can't aggregate the historical event log.
export async function getLiquidations(
  query: LiquidationsQuery,
): Promise<IndexerPositionEventRow[]> {
  const params = new URLSearchParams()
  if (query.mint) params.set('mint', query.mint)
  if (query.owner) params.set('owner', query.owner)
  if (query.side) params.set('side', query.side)
  if (query.kind) params.set('kind', query.kind)
  if (query.limit != null) params.set('limit', String(query.limit))
  const qs = params.toString()
  return indexerFetch<IndexerPositionEventRow[]>(
    query.indexer,
    qs ? `/api/liquidations?${qs}` : '/api/liquidations',
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
  return indexerFetch<IndexerCandle[]>(query.indexer, `/api/candles?${params.toString()}`)
}

/**
 * Post-migration DEX swap rows (deep_pool `swaps` table). Use this alongside
 * `getTrades` (which queries the bonding-curve `trades` table) to assemble
 * the full lifetime trade history for a mint.
 *
 * Indexer-only.
 */
export async function getSwaps(query: SwapsQuery): Promise<IndexerSwapRow[]> {
  const params = new URLSearchParams()
  if (query.tokenMint) params.set('token_mint', query.tokenMint)
  if (query.poolId != null) params.set('pool_id', String(query.poolId))
  if (query.user) params.set('user', query.user)
  if (query.since) params.set('since', query.since.toISOString())
  if (query.before) params.set('before', query.before.toISOString())
  if (query.limit != null) params.set('limit', String(query.limit))
  const qs = params.toString()
  return indexerFetch<IndexerSwapRow[]>(query.indexer, qs ? `/api/swaps?${qs}` : '/api/swaps')
}

/**
 * Realized PnL for a wallet across all mints they've traded, computed via
 * FIFO cost basis over their full bonding-curve and DEX swap history. The
 * indexer aggregates server-side; this is a thin HTTP wrapper.
 *
 * Unrealized PnL is NOT computed server-side — clients should multiply
 * `tokens_remaining` by the current marginal spot price (available from
 * the markets/pools endpoints) and subtract `cost_basis_remaining`.
 *
 * Indexer-only — no RPC equivalent.
 */
export async function getUserPnl(indexer: string, wallet: string): Promise<UserPnlSummary> {
  return indexerFetch<UserPnlSummary>(indexer, `/api/user-pnl/${wallet}`)
}
