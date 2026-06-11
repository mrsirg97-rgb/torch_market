/**
 * Token data fetching
 *
 * Read-only functions for querying token state from Solana.
 */

import { AccountInfo, Connection, PublicKey } from '@solana/web3.js'
import { BorshCoder, Idl } from '@coral-xyz/anchor'
import {
  ExtensionType,
  getAssociatedTokenAddressSync,
  getExtensionData,
  getTokenMetadata as splGetTokenMetadata,
  unpackMint,
} from '@solana/spl-token'
import {
  BondingCurve,
  Treasury,
  TorchVault,
  VaultWalletLink,
  Position,
  UserStats,
  ProtocolTreasury,
  getBondingCurvePda,
  getTokenTreasuryPda,
  getTreasurySolVaultPda,
  getPositionPda,
  getShortVaultPda,
  getPositionTokenVault,
  getTorchVaultPda,
  getVaultSolPda,
  getVaultWalletLinkPda,
  getUserStatsPda,
  getProtocolTreasuryPda,
  getDeepPoolAccounts,
  calculateBondingProgress,
  calculatePrice,
  getDepthMaxLtvBps,
} from './program'
import {
  PROGRAM_ID,
  BLACKLISTED_MINTS,
  LAMPORTS_PER_SOL,
  TOKEN_MULTIPLIER,
  TOTAL_SUPPLY,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_DECIMALS,
  MEMO_PROGRAM_ID,
} from './constants'
import idl from './torch_market.json'
import {
  TokenSummary,
  TokenDetail,
  TokenStatus,
  TokenListParams,
  TokenListResult,
  TokenPageParams,
  TokenPageResult,
  Holder,
  HoldersResult,
  TokenMessage,
  MessagesResult,
  SaidVerification,
  LendingInfo,
  PositionInfo,
  PositionWithKey,
  AllPositionsResult,
  PositionSide,
  VaultInfo,
  VaultWalletLinkInfo,
  UserStatsInfo,
  ProtocolTreasuryInfo,
  TreasuryInfo,
  TokenMetadataResult,
  ReadOptions,
} from './types'
import {
  fetchPositionsFromIndexer,
  fetchMessagesFromIndexer,
  fetchMarketsFromIndexer,
  withFallback,
  type IndexerMarketStatus,
  type IndexerPositionRow,
} from './indexer'

// ============================================================================
// Internal helpers
// ============================================================================

interface RawToken {
  mint: string
  bondingCurve: BondingCurve
}

export interface MintMetadata {
  name: string
  symbol: string
  uri: string
}

// Parses a TokenMetadata extension TLV payload. Layout:
// updateAuthority(32) + mint(32) + name(u32-len + utf8) + symbol + uri + additionalMetadata
// (the extension header is already stripped by getExtensionData).
const parseTokenMetadataTlv = (buf: Buffer): MintMetadata => {
  let offset = 64 // skip updateAuthority(32) + mint(32)
  const readString = (): string => {
    const len = buf.readUInt32LE(offset)
    offset += 4
    const s = buf.slice(offset, offset + len).toString('utf-8')
    offset += len
    return s
  }
  return { name: readString(), symbol: readString(), uri: readString() }
}

const parseMintMetadataFromAccount = (
  mint: PublicKey,
  info: AccountInfo<Buffer>,
): MintMetadata | null => {
  try {
    const mintData = unpackMint(mint, info, TOKEN_2022_PROGRAM_ID)
    const metadataBytes = getExtensionData(ExtensionType.TokenMetadata, mintData.tlvData)
    if (!metadataBytes) return null
    return parseTokenMetadataTlv(Buffer.from(metadataBytes))
  } catch {
    return null
  }
}

// Batch-fetch Token-2022 metadata extensions for many mints in one (or few) RPCs.
// getMultipleAccountsInfo caps at 100 accounts per call, so larger lists are chunked.
export const fetchMintsMetadata = async (
  connection: Connection,
  mints: PublicKey[],
): Promise<Map<string, MintMetadata>> => {
  const map = new Map<string, MintMetadata>()
  if (mints.length === 0) return map
  const chunks: PublicKey[][] = []
  for (let i = 0; i < mints.length; i += 100) chunks.push(mints.slice(i, i + 100))
  const results = await Promise.all(chunks.map((c) => connection.getMultipleAccountsInfo(c)))
  for (let ci = 0; ci < chunks.length; ci++) {
    const chunk = chunks[ci]
    const infos = results[ci]
    for (let i = 0; i < chunk.length; i++) {
      const info = infos[i]
      if (!info) continue
      const md = parseMintMetadataFromAccount(chunk[i], info as AccountInfo<Buffer>)
      if (md) map.set(chunk[i].toBase58(), md)
    }
  }
  return map
}

const getTokenStatus = (bc: BondingCurve): TokenStatus => {
  if (bc.reclaimed) return 'reclaimed'
  if (bc.migrated) return 'migrated'
  if (bc.bonding_complete) return 'complete'
  return 'bonding'
}

// Fetch DeepPool reserves: SOL from pool PDA lamports (minus rent), tokens from vault balance.
// Uses the pool account's actual data length for rent calculation — mirrors deeppoolsdk's pattern
// so the SDK never drifts when Pool::LEN changes on-chain.
const fetchDeepPoolReserves = async (
  connection: Connection,
  mint: PublicKey,
): Promise<{ solReserves: number; tokenReserves: number }> => {
  const deepPool = getDeepPoolAccounts(mint)
  const poolInfo = await connection.getAccountInfo(deepPool.pool)
  if (!poolInfo) throw new Error('DeepPool not found')
  const [vaultBalance, rentExempt] = await Promise.all([
    connection.getTokenAccountBalance(deepPool.tokenVault),
    connection.getMinimumBalanceForRentExemption(poolInfo.data.length),
  ])
  const solReserves = poolInfo.lamports - rentExempt
  const tokenReserves = Number(vaultBalance.value.amount)
  return { solReserves, tokenReserves }
}

const fetchAllRawTokens = async (connection: Connection): Promise<RawToken[]> => {
  const coder = new BorshCoder(idl as unknown as Idl)

  const accounts = await connection.getProgramAccounts(PROGRAM_ID, {
    filters: [{ memcmp: { offset: 0, bytes: '4y6pru6YvC7' } }],
  })

  const tokens: RawToken[] = []

  for (const acc of accounts) {
    try {
      const decoded = coder.accounts.decode('BondingCurve', acc.account.data)
      const mintStr = decoded.mint.toString()

      if (BLACKLISTED_MINTS.includes(mintStr)) continue

      tokens.push({
        mint: mintStr,
        bondingCurve: decoded as unknown as BondingCurve,
      })
    } catch {
      // Not a bonding curve account
    }
  }

  return tokens
}

const toTokenSummary = (raw: RawToken, meta?: MintMetadata): TokenSummary => {
  const bc = raw.bondingCurve

  const virtualSol = BigInt(bc.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bc.virtual_token_reserves.toString())
  const realSol = BigInt(bc.real_sol_reserves.toString())
  const realTokens = BigInt(bc.real_token_reserves.toString())

  const price = calculatePrice(virtualSol, virtualTokens)
  const priceInSol = (price * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL

  // Market cap = fully diluted (total supply × price), matching pump.fun convention
  const marketCapSol = (priceInSol * Number(TOTAL_SUPPLY)) / TOKEN_MULTIPLIER

  return {
    mint: raw.mint,
    name: meta?.name ?? '',
    symbol: meta?.symbol ?? '',
    status: getTokenStatus(bc),
    price_sol: priceInSol,
    market_cap_sol: marketCapSol,
    progress_percent: calculateBondingProgress(realSol),
    holders: null,
    created_at: 0,
    last_activity_at: Number(bc.last_activity_slot.toString()),
  }
}

// Indexer row → TokenSummary. Fields the indexer exposes get passed through;
// fields it doesn't (holders count) are null, matching the RPC path's
// initial-render state. `image_url` is preserved if the indexer's metadata
// fetcher has populated it — that's a strict improvement over RPC, which
// has no image at all without a per-mint arweave fetch.
//
// Populates the optional enrichment fields (image, creator, bonding_target,
// tier) so the frontend's per-mint enrichment loop can skip mints the
// indexer already enriched — the major perf win at scale.
const indexerRowToTokenSummary = (
  row: import('./indexer').IndexerMarketRow,
  meta?: MintMetadata,
): TokenSummary => {
  const virtualSol = BigInt(row.virtual_sol)
  const virtualTokens = BigInt(row.virtual_token)
  const realSol = BigInt(row.real_sol)
  const solTarget = BigInt(row.sol_target)

  const price = calculatePrice(virtualSol, virtualTokens)
  const priceInSol = (price * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
  const marketCapSol = (priceInSol * Number(TOTAL_SUPPLY)) / TOKEN_MULTIPLIER

  // Indexer status (RS/RD/ASN/MIGRATED/RECLAIMED) → SDK status enum.
  // RD and ASN both mean "bonding complete, awaiting migration" from the
  // consumer's perspective — collapsed into 'complete' to match the
  // frontend's 4-state TokenStatus model.
  let status: TokenStatus
  switch (row.status) {
    case 'MIGRATED':
      status = 'migrated'
      break
    case 'RECLAIMED':
      status = 'reclaimed'
      break
    case 'RD':
    case 'ASN':
      status = 'complete'
      break
    case 'RS':
    default:
      status = 'bonding'
      break
  }

  return {
    mint: row.mint,
    name: meta?.name ?? row.name,
    symbol: meta?.symbol ?? row.symbol,
    status,
    price_sol: priceInSol,
    market_cap_sol: marketCapSol,
    // Pass sol_target so progress is correct for non-default tiers
    // (flame 100 SOL, torch 200; default fallback is 200).
    progress_percent: calculateBondingProgress(realSol, solTarget),
    holders: null,
    created_at: Math.floor(new Date(row.created_at).getTime() / 1000),
    last_activity_at: row.last_activity_slot,
    // Enrichment fields — populated when present, undefined otherwise so
    // the frontend can detect and skip per-mint enrichment.
    image: row.image_url ?? undefined,
    creator: row.creator,
    bonding_target: row.sol_target,
    tier: row.tier,
  }
}

const filterAndSort = (tokens: RawToken[], params: TokenListParams): RawToken[] => {
  let filtered = [...tokens]

  if (params.status && params.status !== 'all') {
    filtered = filtered.filter((t) => getTokenStatus(t.bondingCurve) === params.status)
  }

  switch (params.sort) {
    case 'marketcap':
    case 'volume':
      filtered.sort((a, b) => {
        const aR = BigInt(a.bondingCurve.real_sol_reserves.toString())
        const bR = BigInt(b.bondingCurve.real_sol_reserves.toString())
        return bR > aR ? 1 : bR < aR ? -1 : 0
      })
      break
    case 'newest':
    default:
      filtered.sort((a, b) => {
        const aA = BigInt(a.bondingCurve.last_activity_slot.toString())
        const bA = BigInt(b.bondingCurve.last_activity_slot.toString())
        return bA > aA ? 1 : bA < aA ? -1 : 0
      })
      break
  }

  if (params.limit || params.offset) {
    const offset = params.offset || 0
    const limit = params.limit || filtered.length
    return filtered.slice(offset, offset + limit)
  }
  return filtered
}

const buildTokenDetail = (
  mint: string,
  bc: BondingCurve,
  treasury: Treasury | null,
  mintMeta?: MintMetadata,
  metadata?: {
    description?: string
    image?: string
    twitter?: string
    telegram?: string
    website?: string
  },
  holdersCount?: number | null,
  solPriceUsd?: number,
  saidVerification?: SaidVerification | null,
  warnings?: string[],
  poolPrice?: { solReserves: number; tokenReserves: number },
): TokenDetail => {
  const virtualSol = BigInt(bc.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bc.virtual_token_reserves.toString())
  const realSol = BigInt(bc.real_sol_reserves.toString())
  const realTokens = BigInt(bc.real_token_reserves.toString())

  let priceInSol: number
  let marketCapSol: number

  if (bc.migrated && poolPrice && poolPrice.tokenReserves > 0) {
    // Use live DeepPool price for migrated tokens
    // solReserves is in lamports, tokenReserves is in base units (10^6)
    priceInSol =
      (poolPrice.solReserves * TOKEN_MULTIPLIER) / (poolPrice.tokenReserves * LAMPORTS_PER_SOL)
  } else {
    // Use bonding curve virtual reserves for pre-migration tokens
    const price = calculatePrice(virtualSol, virtualTokens)
    priceInSol = (price * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
  }

  // Market cap = fully diluted (total supply × price), matching pump.fun convention
  marketCapSol = (priceInSol * Number(TOTAL_SUPPLY)) / TOKEN_MULTIPLIER
  const circulating = TOTAL_SUPPLY - realTokens

  // [V21] Lendable SOL no longer lives on the Treasury data account — it's in
  // the System-owned treasury_sol_vault PDA. Use getLendingInfo() for that
  // balance; the token-detail enrichment leaves it 0 to avoid an extra RPC.
  const treasurySol = 0
  // [V21] Star/creator-reward feature removed — always 0 (kept for API compat).
  const stars = 0

  return {
    mint,
    name: mintMeta?.name ?? '',
    symbol: mintMeta?.symbol ?? '',
    description: metadata?.description,
    image: metadata?.image,
    status: getTokenStatus(bc),
    price_sol: priceInSol,
    price_usd: solPriceUsd ? priceInSol * solPriceUsd : undefined,
    market_cap_sol: marketCapSol,
    market_cap_usd: solPriceUsd ? marketCapSol * solPriceUsd : undefined,
    progress_percent: calculateBondingProgress(realSol),
    sol_raised: Number(realSol) / LAMPORTS_PER_SOL,
    sol_target: 200,
    total_supply: Number(TOTAL_SUPPLY) / TOKEN_MULTIPLIER,
    circulating_supply: Number(circulating) / TOKEN_MULTIPLIER,
    tokens_in_curve: Number(realTokens) / TOKEN_MULTIPLIER,
    tokens_burned: 0,
    treasury_sol_balance: treasurySol,
    treasury_token_balance: 0,
    creator: bc.creator.toString(),
    holders: holdersCount ?? null,
    stars,
    created_at: 0,
    last_activity_at: Number(bc.last_activity_slot.toString()),
    twitter: metadata?.twitter,
    telegram: metadata?.telegram,
    website: metadata?.website,
    creator_verified: saidVerification?.verified,
    creator_trust_tier: saidVerification?.trustTier,
    creator_said_name: saidVerification?.name,
    creator_badge_url: saidVerification?.verified
      ? `https://api.saidprotocol.com/api/badge/${bc.creator.toString()}.svg`
      : undefined,
    ...(warnings && warnings.length > 0 ? { warnings } : {}),
  }
}

// Internal: fetch single token on-chain data
const fetchTokenRaw = async (
  connection: Connection,
  mint: PublicKey,
): Promise<{ bondingCurve: BondingCurve; treasury: Treasury | null } | null> => {
  const coder = new BorshCoder(idl as unknown as Idl)

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)

  const [bcAccount, treasuryAccount] = await Promise.all([
    connection.getAccountInfo(bondingCurvePda),
    connection.getAccountInfo(treasuryPda),
  ])

  if (!bcAccount) return null

  const bondingCurve = coder.accounts.decode(
    'BondingCurve',
    bcAccount.data,
  ) as unknown as BondingCurve

  let treasury: Treasury | null = null
  if (treasuryAccount) {
    treasury = coder.accounts.decode('Treasury', treasuryAccount.data) as unknown as Treasury
  }

  return { bondingCurve, treasury }
}

// ============================================================================
// Public API
// ============================================================================

/**
 * List tokens with optional filtering and sorting.
 *
 * Indexer-first when `options.indexer` is provided. The indexer replaces
 * the expensive `getProgramAccounts` scan (which decodes every BondingCurve
 * on-chain) with a single HTTP request — by far the hottest acceleration
 * point in the SDK. Falls back silently to the RPC path on any indexer
 * failure.
 *
 * Per-mint name/symbol still comes from the Token-2022 metadata extension
 * via `fetchMintsMetadata` in both paths, because the indexer's
 * `markets.name`/`symbol` are seeded from the MarketCreated event and may
 * lag if mint metadata was updated post-creation.
 */
export const getTokens = async (
  connection: Connection,
  params: TokenListParams = {},
  options?: ReadOptions,
): Promise<TokenListResult> => {
  if (options?.indexer) {
    try {
      return await getTokensViaIndexer(connection, params, options.indexer)
    } catch {
      // fall through to RPC path
    }
  }

  const allTokens = await fetchAllRawTokens(connection)
  const filtered = filterAndSort(allTokens, params)
  const mintKeys = filtered.map((t) => new PublicKey(t.mint))
  const metaMap = await fetchMintsMetadata(connection, mintKeys)
  const summaries = filtered.map((t) => toTokenSummary(t, metaMap.get(t.mint)))

  return {
    tokens: summaries,
    total: allTokens.length,
    limit: params.limit || summaries.length,
    offset: params.offset || 0,
  }
}

// Map the SDK's TokenStatusFilter to the indexer's MarketStatus filter,
// or null for "no status filter" (indexer returns all statuses).
const sdkStatusToIndexer = (s: TokenListParams['status']): IndexerMarketStatus | null => {
  switch (s) {
    case 'bonding':
      return 'RS'
    case 'complete':
      // 'complete' maps to RD on the indexer side (RD = bonding-complete,
      // pre-migration). ASN gets collapsed into 'complete' on read; we
      // don't filter on ASN here because no current writer code path
      // produces it.
      return 'RD'
    case 'migrated':
      return 'MIGRATED'
    case 'reclaimed':
      return 'RECLAIMED'
    case 'all':
    case undefined:
      return null
    default:
      return null
  }
}

async function getTokensViaIndexer(
  connection: Connection,
  params: TokenListParams,
  indexer: string,
): Promise<TokenListResult> {
  const status = sdkStatusToIndexer(params.status)
  const rows = await fetchMarketsFromIndexer(indexer, {
    status: status ?? undefined,
    limit: params.limit,
  })

  // Token-2022 metadata still resolved client-side. The indexer's
  // markets.name/symbol are seeded once at MarketCreated and don't reflect
  // post-creation mint-metadata updates (rare but possible).
  const mintKeys = rows.map((r) => new PublicKey(r.mint))
  const metaMap = await fetchMintsMetadata(connection, mintKeys)
  const summaries = rows.map((r) => indexerRowToTokenSummary(r, metaMap.get(r.mint)))

  return {
    tokens: summaries,
    total: summaries.length,
    limit: params.limit || summaries.length,
    offset: params.offset || 0,
  }
}

// BondingCurve discriminator (first 8 bytes) base58-encoded. Same filter used by fetchAllRawTokens.
const BONDING_CURVE_DISCRIMINATOR_BS58 = '4y6pru6YvC7'
const DEFAULT_PAGE_LIMIT = 10000

interface RpcAccount {
  pubkey: string
  account: {
    data: [string, string]
    owner: string
    lamports: number
    executable: boolean
    rentEpoch: number
  }
}

// Raw JSON-RPC call for methods @solana/web3.js doesn't expose (e.g. getProgramAccountsV2).
// Throws on RPC-level errors.
const rawRpc = async (endpoint: string, method: string, params: unknown[]): Promise<any> => {
  const res = await fetch(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  })
  const json = (await res.json()) as {
    result?: any
    error?: { message?: string; code?: number }
  }
  if (json.error) {
    throw new Error(
      `RPC ${method} failed: ${json.error.message || 'unknown error'} (code ${json.error.code ?? '?'})`,
    )
  }
  return json.result
}

const decodeRawTokensFromAccounts = (accounts: RpcAccount[]): RawToken[] => {
  const coder = new BorshCoder(idl as unknown as Idl)
  const tokens: RawToken[] = []
  for (const acc of accounts) {
    try {
      const data = Buffer.from(acc.account.data[0], 'base64')
      const decoded = coder.accounts.decode('BondingCurve', data)
      const mintStr = decoded.mint.toString()
      if (BLACKLISTED_MINTS.includes(mintStr)) continue
      tokens.push({ mint: mintStr, bondingCurve: decoded as unknown as BondingCurve })
    } catch {
      // Not a BondingCurve account — skip.
    }
  }
  return tokens
}

/**
 * Fetch one page of tokens via RPC `getProgramAccountsV2` (Helius + compatible RPCs).
 *
 * Unlike `getTokens`, this does not scan the entire program per call — it returns a single
 * page with an opaque cursor. The caller composes the loop:
 *
 *   let paginationKey: string | null = null
 *   const map = new Map<string, TokenSummary>()
 *   do {
 *     const page = await getTokensPage(connection, { paginationKey })
 *     for (const t of page.tokens) map.set(t.mint, t)
 *     paginationKey = page.paginationKey
 *   } while (paginationKey)
 *
 * For incremental deltas, pass `changedSinceSlot: previousPage.currentSlot` on the next poll —
 * only accounts modified at or after that slot are returned.
 *
 * Requires an RPC that implements `getProgramAccountsV2` (Helius, Solana Tracker, etc).
 * Falls through with an RPC error on providers that don't support it.
 */
export const getTokensPage = async (
  connection: Connection,
  params: TokenPageParams = {},
): Promise<TokenPageResult> => {
  const limit = Math.max(1, Math.min(params.limit ?? DEFAULT_PAGE_LIMIT, 10000))
  const config: Record<string, unknown> = {
    encoding: 'base64',
    filters: [{ memcmp: { offset: 0, bytes: BONDING_CURVE_DISCRIMINATOR_BS58 } }],
    limit,
    withContext: true,
  }
  if (params.paginationKey) config.paginationKey = params.paginationKey
  if (typeof params.changedSinceSlot === 'number') {
    config.changedSinceSlot = params.changedSinceSlot
  }

  const result = await rawRpc(connection.rpcEndpoint, 'getProgramAccountsV2', [
    PROGRAM_ID.toBase58(),
    config,
  ])

  // withContext: true → { context: { slot }, value: { accounts, paginationKey } }
  const ctx = result?.context
  const value = result?.value
  const rawAccounts = (value?.accounts ?? []) as RpcAccount[]
  const rawTokens = decodeRawTokensFromAccounts(rawAccounts)
  const mintKeys = rawTokens.map((t) => new PublicKey(t.mint))
  const metaMap = await fetchMintsMetadata(connection, mintKeys)
  const summaries = rawTokens.map((t) => toTokenSummary(t, metaMap.get(t.mint)))

  return {
    tokens: summaries,
    paginationKey: (value?.paginationKey ?? null) as string | null,
    currentSlot: Number(ctx?.slot ?? 0),
  }
}

/**
 * Get on-chain Token-2022 metadata for a token.
 *
 * Reads name, symbol, and uri directly from the mint's TokenMetadata extension.
 * Returns null if the mint has no metadata (legacy pre-V29 tokens).
 */
export const getTokenMetadata = async (
  connection: Connection,
  mintStr: string,
): Promise<TokenMetadataResult | null> => {
  const mint = new PublicKey(mintStr)
  const metadata = await splGetTokenMetadata(connection, mint, 'confirmed', TOKEN_2022_PROGRAM_ID)
  if (!metadata) return null
  return {
    name: metadata.name,
    symbol: metadata.symbol,
    uri: metadata.uri,
    mint: mintStr,
  }
}

/**
 * Get detailed info for a single token.
 */
export const getToken = async (connection: Connection, mintStr: string): Promise<TokenDetail> => {
  const mint = new PublicKey(mintStr)
  const tokenData = await fetchTokenRaw(connection, mint)

  if (!tokenData) {
    throw new Error(`Token not found: ${mintStr}`)
  }

  const { bondingCurve, treasury } = tokenData
  const warnings: string[] = []

  // Pull name/symbol/uri from the Token-2022 metadata extension on the mint.
  let mintMeta: MintMetadata | undefined
  try {
    const md = await getTokenMetadata(connection, mintStr)
    if (md) mintMeta = { name: md.name, symbol: md.symbol, uri: md.uri }
  } catch (e) {
    warnings.push(`Mint metadata fetch failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  // Fetch external metadata json from URI
  let metadata:
    | {
        description?: string
        image?: string
        twitter?: string
        telegram?: string
        website?: string
      }
    | undefined
  const uri = mintMeta?.uri
  if (uri) {
    try {
      const controller = new AbortController()
      const timer = setTimeout(() => controller.abort(), 10_000)
      const res = await fetch(uri, { signal: controller.signal }).finally(() => clearTimeout(timer))
      const data = (await res.json()) as Record<string, any>
      metadata = {
        description: data.description,
        image: data.image,
        twitter: data.twitter,
        telegram: data.telegram,
        website: data.website,
      }
    } catch (e) {
      warnings.push(`Metadata fetch failed: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  // Fetch holders count
  let holdersCount: number | null = null
  try {
    const holders = await connection.getTokenLargestAccounts(mint, 'confirmed')
    holdersCount = holders.value.filter((a) => a.uiAmount && a.uiAmount > 0).length
  } catch (e) {
    warnings.push(`Holders fetch failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  // Fetch SOL price
  let solPriceUsd: number | undefined
  try {
    const res = await fetch(
      'https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd',
    )
    const data = (await res.json()) as { solana?: { usd?: number } }
    solPriceUsd = data?.solana?.usd
  } catch (e) {
    warnings.push(`SOL price fetch failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  // Fetch live pool price for migrated tokens
  let poolPrice: { solReserves: number; tokenReserves: number } | undefined
  if (bondingCurve.migrated) {
    try {
      poolPrice = await fetchDeepPoolReserves(connection, mint)
    } catch (e) {
      warnings.push(`Pool price fetch failed: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  return buildTokenDetail(
    mintStr,
    bondingCurve,
    treasury,
    mintMeta,
    metadata,
    holdersCount,
    solPriceUsd,
    undefined,
    warnings,
    poolPrice,
  )
}

/**
 * Get top holders for a token.
 */
export const getHolders = async (
  connection: Connection,
  mintStr: string,
  limit: number = 20,
): Promise<HoldersResult> => {
  const mint = new PublicKey(mintStr)
  const safeLimit = Math.min(limit, 100)

  // Build excluded addresses (pools/vaults)
  const excluded = new Set<string>()

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const bondingCurveVault = getAssociatedTokenAddressSync(
    mint,
    bondingCurvePda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  excluded.add(bondingCurveVault.toString())

  const [treasuryPda] = getTokenTreasuryPda(mint)
  const treasuryVault = getAssociatedTokenAddressSync(
    mint,
    treasuryPda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  excluded.add(treasuryVault.toString())

  try {
    const deepPool = getDeepPoolAccounts(mint)
    excluded.add(deepPool.tokenVault.toString())
  } catch {
    // Ignore
  }

  const response = await connection.getTokenLargestAccounts(mint, 'confirmed')
  const totalSupply = BigInt(1_000_000_000) * BigInt(10 ** TOKEN_DECIMALS)

  const filteredAccounts = response.value
    .filter((account) => account.uiAmount && account.uiAmount > 0)
    .filter((account) => !excluded.has(account.address.toString()))
    .slice(0, safeLimit)

  const accountInfos = await connection.getMultipleParsedAccounts(
    filteredAccounts.map((a) => a.address),
  )

  const holders: Holder[] = filteredAccounts.map((account, i) => {
    const parsed = accountInfos.value[i]?.data
    const owner = parsed && 'parsed' in parsed ? (parsed as any).parsed?.info?.owner : null
    return {
      address: owner || account.address.toString(),
      balance: Number(account.amount) / 10 ** TOKEN_DECIMALS,
      percentage: (Number(account.amount) / Number(totalSupply)) * 100,
    }
  })

  return {
    holders,
    total_holders: response.value.filter(
      (a) => a.uiAmount && a.uiAmount > 0 && !excluded.has(a.address.toString()),
    ).length,
  }
}

/**
 * Get messages (memos) for a token.
 */
export const getMessages = async (
  connection: Connection,
  mintStr: string,
  limit: number = 50,
  opts?: {
    source?: 'bonding' | 'pool' | 'all'
    enrich?: boolean
  } & ReadOptions,
): Promise<MessagesResult> => {
  const mint = new PublicKey(mintStr)
  const safeLimit = Math.min(limit, 100)
  const sigLimit = Math.min(safeLimit, 50)
  const source = opts?.source ?? 'all'

  // Indexer-first path: the indexer stores memos co-resident with torch
  // trades (per the gating policy), pre-decoded. Massively faster than
  // walking getSignaturesForAddress + getParsedTransactions for every sig.
  // Falls back silently to the RPC walker on any indexer failure.
  if (opts?.indexer) {
    try {
      const rows = await fetchMessagesFromIndexer(opts.indexer, mintStr, safeLimit)
      const messages: TokenMessage[] = rows.map((r) => ({
        signature: r.signature,
        memo: r.memo_text,
        sender: r.sender,
        // Indexer's created_at is the slot's block_time (UTC). Frontend
        // expects unix-seconds; convert.
        timestamp: Math.floor(new Date(r.created_at).getTime() / 1000),
      }))
      return { messages, total: messages.length }
    } catch {
      // fall through to RPC walker
    }
  }

  // Helper: extract memo from a parsed transaction
  const extractMemo = async (
    tx: import('@solana/web3.js').ParsedTransactionWithMeta,
    signature: string,
    blockTime: number,
  ): Promise<TokenMessage | null> => {
    const allInstructions = [
      ...tx.transaction.message.instructions,
      ...(tx.meta?.innerInstructions || []).flatMap((inner) => inner.instructions),
    ]

    for (const ix of allInstructions) {
      const programId = 'programId' in ix ? ix.programId.toString() : ''
      const programName = 'program' in ix ? (ix as { program: string }).program : ''

      const isMemo =
        programId === MEMO_PROGRAM_ID.toString() ||
        programId === 'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr' ||
        programName === 'spl-memo'

      if (isMemo) {
        let memoText = ''

        if ('parsed' in ix) {
          memoText = typeof ix.parsed === 'string' ? ix.parsed : JSON.stringify(ix.parsed)
        } else if ('data' in ix && typeof ix.data === 'string') {
          try {
            const bs58 = await import('bs58')
            const decoded = bs58.default.decode(ix.data)
            memoText = new TextDecoder().decode(decoded)
          } catch {
            memoText = ix.data
          }
        }

        if (memoText && memoText.trim()) {
          const sender = tx.transaction.message.accountKeys[0]?.pubkey?.toString() || 'Unknown'
          return {
            signature,
            memo: memoText.trim(),
            sender,
            timestamp: blockTime,
          }
        }
      }
    }
    return null
  }

  // Helper: fetch messages from a given account's signatures
  const fetchMessagesFromAccount = async (account: PublicKey): Promise<TokenMessage[]> => {
    const signatures = await connection.getSignaturesForAddress(
      account,
      { limit: sigLimit },
      'confirmed',
    )

    if (signatures.length === 0) return []

    const result: TokenMessage[] = []
    const BATCH_SIZE = 100

    for (let i = 0; i < signatures.length && result.length < safeLimit; i += BATCH_SIZE) {
      const batch = signatures.slice(i, i + BATCH_SIZE)
      const sigStrings = batch.map((s) => s.signature)

      let txs: (import('@solana/web3.js').ParsedTransactionWithMeta | null)[]
      try {
        txs = await connection.getParsedTransactions(sigStrings, {
          maxSupportedTransactionVersion: 0,
        })
      } catch {
        continue
      }

      for (let j = 0; j < txs.length && result.length < safeLimit; j++) {
        const tx = txs[j]
        if (!tx?.meta || tx.meta.err) continue

        const msg = await extractMemo(tx, batch[j].signature, batch[j].blockTime || 0)
        if (msg) result.push(msg)
      }
    }

    return result
  }

  // Fetch from requested source(s)
  let bondingMessages: TokenMessage[] = []
  let poolMessages: TokenMessage[] = []

  if (source === 'bonding' || source === 'all') {
    const [bondingCurvePda] = getBondingCurvePda(mint)
    bondingMessages = await fetchMessagesFromAccount(bondingCurvePda).catch(() => [])
  }

  if (source === 'pool' || source === 'all') {
    try {
      const { pool } = getDeepPoolAccounts(mint)
      poolMessages = await fetchMessagesFromAccount(pool)
    } catch {
      // no pool
    }
  }

  // Merge, dedupe by signature, sort newest first, trim to limit
  const seen = new Set<string>()
  const messages: TokenMessage[] = []

  for (const m of [...bondingMessages, ...poolMessages]) {
    if (!seen.has(m.signature)) {
      seen.add(m.signature)
      messages.push(m)
    }
  }

  messages.sort((a, b) => b.timestamp - a.timestamp)

  const trimmed = messages.slice(0, safeLimit)

  // Enrich with SAID verification when opted in
  if (opts?.enrich) {
    const { verifySaid } = await import('./said')
    const uniqueSenders = [...new Set(trimmed.map((m) => m.sender))]
    const verifications = await Promise.all(
      uniqueSenders.map(async (sender) => {
        try {
          const v = await verifySaid(sender)
          return [sender, v] as const
        } catch {
          return [sender, null] as const
        }
      }),
    )
    const verifyMap = new Map(verifications)
    for (const msg of trimmed) {
      const v = verifyMap.get(msg.sender)
      if (v) {
        msg.sender_verified = v.verified
        msg.sender_trust_tier = v.trustTier
        msg.sender_said_name = v.name
        if (v.verified) {
          msg.sender_badge_url = `https://api.saidprotocol.com/api/badge/${msg.sender}.svg`
        }
      }
    }
  }

  return { messages: trimmed, total: trimmed.length }
}

// ============================================================================
// Lending (V2.4)
// ============================================================================

// Lending constants (matching the Rust program — see programs/torch_market/src/constants.rs)
const INTEREST_RATE_BPS = 150 // [V21] 1.5% per epoch (was 200)
const LIQUIDATION_THRESHOLD_BPS = 6500 // 65%
const LIQUIDATION_BONUS_BPS = 3250 // [V21] 32.5% ceiling = 1.3·ρ_max (was 1000)
const LENDING_UTILIZATION_CAP_BPS = 8000 // 80% (V4.0, was 70%)
const BORROW_SHARE_MULTIPLIER = 23 // Per-user cap: max borrow = 23x collateral share of supply (V10.2.5, was 5x)

/**
 * Token-2022 transfer fee on this protocol's mint (7 bps = 0.07%). Mirrors
 * the on-chain `TRANSFER_FEE_BPS` constant.
 */
export const TRANSFER_FEE_BPS = 7

/**
 * v20.0.0: gross-up a NET token amount by the Token-2022 transfer fee so
 * the recipient receives the full net after withholding. Mirror of the
 * on-chain `math::gross_up_for_transfer_fee` helper.
 *
 * Used on the short close+liquidate flows: the on-chain program transfers
 * `gross_up_for_transfer_fee(token_amount)` from the borrower so
 * treasury_lock receives the full `token_amount` net (token pool stable).
 * Frontends should call this to compute the wallet balance required to
 * fully close a position.
 *
 * Formula matches the on-chain ceiling division:
 *   gross = ceil(net × 10000 / (10000 − TRANSFER_FEE_BPS))
 *
 * @param net - desired net token amount the recipient should receive
 * @returns gross token amount the sender must transfer
 */
export const grossUpForTransferFee = (net: number): number => {
  const denom = 10_000 - TRANSFER_FEE_BPS
  return Math.ceil((net * 10_000) / denom)
}
// V20.0.0: absolute per-user ceiling. min(formula × 23 / supply, max_lendable × 0.2)
// for any collateral size. Without this, a user with > ~4.35% of supply could
// take the entire lendable amount.
const MAX_USER_BORROW_SHARE_BPS = 2000 // 20% of max_lendable
// V20.0.0: lending unlock gate on treasury.sol_balance. Mainnet only — devnet
// builds the program with 1 SOL gate, simnet with 0. SDK defaults to mainnet
// (100 SOL); frontend can override per network via the optional parameter
// on getBorrowQuote / getLendingInfo.
const DEFAULT_LENDING_UNLOCK_THRESHOLD_LAMPORTS = 100_000_000_000 // 100 SOL
const EPOCH_DURATION_SLOTS = 1_512_000 // 7 days at 400ms/slot — matches on-chain EPOCH_DURATION_SLOTS

// Project simple-linear interest forward to the given slot, matching the on-chain
// accrue_interest() formula exactly:
//   interest = principal * rate_bps * slots_elapsed / (10000 * EPOCH_DURATION_SLOTS)
// See programs/torch_market/src/handlers/lending.rs:accrue_interest.
// Returns total accrued interest (stored + projected pending), not just the delta.
const projectAccruedInterest = (
  principal: number,
  storedAccrued: number,
  lastUpdateSlot: number,
  currentSlot: number,
  rateBps: number = INTEREST_RATE_BPS,
): number => {
  if (principal <= 0) return storedAccrued
  const slotsElapsed = Math.max(0, currentSlot - lastUpdateSlot)
  if (slotsElapsed === 0) return storedAccrued
  // Use BigInt to match on-chain u128 math and avoid precision loss at high slot counts.
  const delta = Number(
    (BigInt(principal) * BigInt(rateBps) * BigInt(slotsElapsed)) /
      (BigInt(10_000) * BigInt(EPOCH_DURATION_SLOTS)),
  )
  return storedAccrued + delta
}

// [V21] Depth-scaled max LTV (continuous concave curve) — imported from program.ts
// (shared with quotes.ts), mirror of pool_validation::get_depth_max_ltv_bps.

/**
 * [V21] Lending / leverage info for a migrated token.
 *
 * Rates + LTV limits + the long/short custody snapshot. Lendable SOL is read
 * from the System-owned treasury_sol_vault PDA lamports (the Treasury data
 * account is accounting-only).
 */
export const getLendingInfo = async (
  connection: Connection,
  mintStr: string,
): Promise<LendingInfo> => {
  const mint = new PublicKey(mintStr)

  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) throw new Error(`Token not found: ${mintStr}`)

  const { bondingCurve, treasury } = tokenData
  if (!bondingCurve.migrated) throw new Error('Token not yet migrated, lending not available')
  if (!treasury) throw new Error(`Treasury not found for ${mintStr}`)

  // Lendable SOL lives in the System-owned treasury_sol_vault (lamports).
  const [treasurySolVaultPda] = getTreasurySolVaultPda(mint)
  let treasurySolVaultLamports = 0
  try {
    const vaultInfo = await connection.getAccountInfo(treasurySolVaultPda)
    treasurySolVaultLamports = vaultInfo?.lamports ?? 0
  } catch {
    // leave 0 on failure
  }

  return {
    interest_rate_bps: treasury.interest_rate_bps,
    max_ltv_bps: treasury.max_ltv_bps,
    liquidation_threshold_bps: treasury.liquidation_threshold_bps,
    liquidation_bonus_bps: treasury.liquidation_bonus_bps,
    liquidation_close_bps: treasury.liquidation_close_bps,
    lending_enabled: treasury.lending_enabled,
    short_selling_enabled: treasury.short_selling_enabled,
    treasury_sol_vault_lamports: treasurySolVaultLamports,
    total_sol_lent_to_longs: Number(treasury.total_sol_lent_to_longs.toString()),
    active_longs: Number(treasury.active_longs.toString()),
    total_tokens_lent: Number(treasury.total_tokens_lent.toString()),
    active_shorts: Number(treasury.active_shorts.toString()),
    total_token_collateral_locked: Number(treasury.total_token_collateral_locked.toString()),
  }
}

// Position account on-chain layout (after the 8-byte discriminator):
//   user(32), mint(32), side(1), position_index(4), collateral_amount(8),
//   debt_amount(8), accrued_interest(8), last_slot(8), bump(1), vault_bump(1)
const POSITION_MINT_OFFSET = 8 + 32 // 40
const POSITION_SIDE_OFFSET = 8 + 32 + 32 // 72

const decodeSide = (side: unknown): PositionSide =>
  side && typeof side === 'object' && 'short' in (side as Record<string, unknown>)
    ? 'short'
    : 'long'

// Compute health for a position given live pool reserves. Unit-by-side:
//   short → collateral = SOL, debt = tokens (debt valued via price)
//   long  → collateral = tokens (valued via price), debt = SOL
const computePositionHealth = (
  side: PositionSide,
  collateralAmount: number,
  debtAmount: number,
  interest: number,
  solReserves: number,
  tokenReserves: number,
): {
  totalOwed: number
  debtValueSol: number | null
  currentLtvBps: number | null
  health: PositionInfo['health']
} => {
  const totalOwed = debtAmount + interest
  const havePrice = tokenReserves > 0 && solReserves > 0
  const price = havePrice ? solReserves / tokenReserves : 0

  let debtValueSol: number | null
  let collateralValueSol: number | null
  if (!havePrice) {
    debtValueSol = null
    collateralValueSol = null
  } else if (side === 'short') {
    debtValueSol = totalOwed * price
    collateralValueSol = collateralAmount // already SOL
  } else {
    debtValueSol = totalOwed // already SOL
    collateralValueSol = collateralAmount * price
  }

  let currentLtvBps: number | null
  if (debtValueSol === null || collateralValueSol === null) {
    currentLtvBps = null
  } else if (collateralValueSol > 0) {
    currentLtvBps = Math.floor((debtValueSol / collateralValueSol) * 10000)
  } else {
    currentLtvBps = totalOwed > 0 ? 10000 : 0
  }

  const maxLtvBps = getDepthMaxLtvBps(solReserves)
  let health: PositionInfo['health']
  if (debtAmount === 0 && interest === 0) {
    health = 'none'
  } else if (currentLtvBps === null) {
    health = 'healthy'
  } else if (currentLtvBps >= LIQUIDATION_THRESHOLD_BPS) {
    health = 'liquidatable'
  } else if (currentLtvBps >= maxLtvBps) {
    health = 'at_risk'
  } else {
    health = 'healthy'
  }
  return { totalOwed, debtValueSol, currentLtvBps, health }
}

// [V21] The collateral base the on-chain liquidation trigger uses is the LIVE
// per-position vault balance, NOT the stored `collateral_amount`:
//   short → position_sol_vault.lamports()  (posted SOL + sale proceeds)
//   long  → position_token_vault.amount    (posted + bought tokens)
// Read it so SDK LTV/health matches the program exactly. Falls back to the
// posted amount if the vault can't be read.
const fetchLiveCollateral = async (
  connection: Connection,
  mint: PublicKey,
  owner: PublicKey,
  side: PositionSide,
  positionIndex: number,
  positionPda: PublicKey,
  fallback: number,
): Promise<number> => {
  try {
    if (side === 'short') {
      const [solVault] = getShortVaultPda(owner, mint, positionIndex)
      const info = await connection.getAccountInfo(solVault)
      return info?.lamports ?? fallback
    }
    const bal = await connection.getTokenAccountBalance(getPositionTokenVault(mint, positionPda))
    return Number(bal.value.amount)
  } catch {
    return fallback
  }
}

/**
 * [V21] Get a single leverage position (replaces getLoanPosition +
 * getShortPosition). Reads the Position PDA on-chain and computes live health
 * against DeepPool reserves. Returns health='none' when the position is empty.
 */
export const getPosition = async (
  connection: Connection,
  mintStr: string,
  ownerStr: string,
  side: PositionSide,
  positionIndex = 0,
): Promise<PositionInfo> => {
  const mint = new PublicKey(mintStr)
  const owner = new PublicKey(ownerStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [positionPda] = getPositionPda(owner, mint, side, positionIndex)
  const [accountInfo, currentSlot] = await Promise.all([
    connection.getAccountInfo(positionPda),
    connection.getSlot('confirmed'),
  ])

  if (!accountInfo) {
    return {
      side,
      position_index: positionIndex,
      collateral_amount: 0,
      debt_amount: 0,
      accrued_interest: 0,
      accrued_interest_stored: 0,
      last_update_slot: 0,
      total_owed: 0,
      debt_value_sol: 0,
      current_ltv_bps: 0,
      health: 'none',
      owner_is_vault: false,
    }
  }

  const pos = coder.accounts.decode('Position', accountInfo.data) as unknown as Position
  const postedCollateral = Number(pos.collateral_amount.toString())
  const debt = Number(pos.debt_amount.toString())
  const storedInterest = Number(pos.accrued_interest.toString())
  const lastUpdateSlot = Number(pos.last_slot.toString())
  const interest = projectAccruedInterest(debt, storedInterest, lastUpdateSlot, currentSlot)

  let solReserves = 0
  let tokenReserves = 0
  const warnings: string[] = []
  try {
    const reserves = await fetchDeepPoolReserves(connection, mint)
    solReserves = reserves.solReserves
    tokenReserves = reserves.tokenReserves
  } catch (e) {
    warnings.push(`Pool valuation failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  // [V21] The on-chain liquidation trigger sizes LTV against the LIVE per-position
  // vault, not the posted collateral: a short's position_sol_vault also holds the
  // SOL proceeds from selling the borrowed tokens; a long's position_token_vault
  // holds posted + bought tokens. Read the vault so SDK health matches the program.
  const collateral = await fetchLiveCollateral(
    connection,
    mint,
    owner,
    side,
    positionIndex,
    positionPda,
    postedCollateral,
  )

  const { totalOwed, debtValueSol, currentLtvBps, health } = computePositionHealth(
    side,
    collateral,
    debt,
    interest,
    solReserves,
    tokenReserves,
  )

  return {
    side,
    position_index: positionIndex,
    collateral_amount: collateral,
    debt_amount: debt,
    accrued_interest: interest,
    accrued_interest_stored: storedInterest,
    last_update_slot: lastUpdateSlot,
    total_owed: totalOwed,
    debt_value_sol: debtValueSol,
    current_ltv_bps: currentLtvBps,
    health,
    owner_is_vault: false,
    ...(warnings.length > 0 ? { warnings } : {}),
  }
}

// Row shape shared by the RPC-scan and indexer-fetch paths.
interface RawPositionRow {
  owner: string
  side: PositionSide
  position_index: number
  collateral_amount: number
  debt_amount: number
  accrued_interest_stored: number
  last_update_slot: number
  owner_is_vault: boolean
}

async function fetchPositionRowsViaRpc(
  connection: Connection,
  mint: PublicKey,
  side?: PositionSide,
): Promise<RawPositionRow[]> {
  const coder = new BorshCoder(idl as unknown as Idl)
  const positionDiscriminator = coder.accounts.accountDiscriminator('Position')
  const bs58 = await import('bs58')
  const filters: { memcmp: { offset: number; bytes: string } }[] = [
    { memcmp: { offset: 0, bytes: bs58.default.encode(positionDiscriminator) } },
    { memcmp: { offset: POSITION_MINT_OFFSET, bytes: mint.toBase58() } },
  ]
  if (side) {
    const sideByte = side === 'short' ? 1 : 0
    filters.push({
      memcmp: {
        offset: POSITION_SIDE_OFFSET,
        bytes: bs58.default.encode(Uint8Array.from([sideByte])),
      },
    })
  }
  const accounts = await connection.getProgramAccounts(PROGRAM_ID, { filters })
  const rows: RawPositionRow[] = []
  for (const acc of accounts) {
    try {
      const pos = coder.accounts.decode('Position', acc.account.data) as unknown as Position
      const debt = Number(pos.debt_amount.toString())
      if (debt > 0) {
        rows.push({
          owner: pos.user.toString(),
          side: decodeSide(pos.side),
          position_index: pos.position_index,
          collateral_amount: Number(pos.collateral_amount.toString()),
          debt_amount: debt,
          accrued_interest_stored: Number(pos.accrued_interest.toString()),
          last_update_slot: Number(pos.last_slot.toString()),
          owner_is_vault: false,
        })
      }
    } catch {
      // Skip malformed accounts.
    }
  }
  return rows
}

async function fetchPositionRowsViaIndexer(
  indexer: string,
  mintStr: string,
  side?: PositionSide,
): Promise<RawPositionRow[]> {
  const rows: IndexerPositionRow[] = await fetchPositionsFromIndexer(
    indexer,
    mintStr,
    side ?? null,
    true,
  )
  return rows.map((r) => ({
    owner: r.owner,
    side: r.side,
    position_index: r.position_index,
    collateral_amount: r.collateral_amount,
    debt_amount: r.debt_amount,
    accrued_interest_stored: r.accrued_interest_stored,
    last_update_slot: r.last_update_slot,
    owner_is_vault: r.owner_is_vault,
  }))
}

/**
 * [V21] Get all active leverage positions for a token (replaces
 * getAllLoanPositions + getAllShortPositions). Pass `side` to scope to long or
 * short. Indexer-first when `options.indexer` is set; falls back to an on-chain
 * scan. Both paths feed the same live interest projection + health calc.
 * Sort order: liquidatable first, then at_risk, then healthy.
 */
export const getAllPositions = async (
  connection: Connection,
  mintStr: string,
  options?: ReadOptions & { side?: PositionSide },
): Promise<AllPositionsResult> => {
  const mint = new PublicKey(mintStr)
  const side = options?.side

  const rows = options?.indexer
    ? await withFallback(
        () => fetchPositionRowsViaIndexer(options.indexer!, mintStr, side),
        () => fetchPositionRowsViaRpc(connection, mint, side),
      )
    : await fetchPositionRowsViaRpc(connection, mint, side)

  // Fetch DeepPool reserves + current slot ONCE.
  let poolPriceSol: number | null = null
  let solReserves = 0
  let tokenReserves = 0
  let currentSlot = 0
  try {
    const [reserves, slot] = await Promise.all([
      fetchDeepPoolReserves(connection, mint),
      connection.getSlot('confirmed'),
    ])
    currentSlot = slot
    solReserves = reserves.solReserves
    tokenReserves = reserves.tokenReserves
    if (tokenReserves > 0) {
      poolPriceSol = solReserves / tokenReserves
    }
  } catch {
    try {
      currentSlot = await connection.getSlot('confirmed')
    } catch {
      /* ignore */
    }
  }

  // [V21] Live per-position vault collateral (the on-chain LTV base — see
  // fetchLiveCollateral). Derive each position's vault and batch the reads.
  //   short → position_sol_vault (lamports);  long → position_token_vault (ATA amount)
  const vaultKeys: PublicKey[] = rows.map((row) => {
    const owner = new PublicKey(row.owner)
    if (row.side === 'short') return getShortVaultPda(owner, mint, row.position_index)[0]
    const [posPda] = getPositionPda(owner, mint, 'long', row.position_index)
    return getPositionTokenVault(mint, posPda)
  })
  const liveCollateral: number[] = rows.map((r) => r.collateral_amount) // fallback to posted
  if (vaultKeys.length > 0) {
    try {
      const chunks: PublicKey[][] = []
      for (let i = 0; i < vaultKeys.length; i += 100) chunks.push(vaultKeys.slice(i, i + 100))
      const results = await Promise.all(
        chunks.map((c) => connection.getMultipleAccountsInfo(c)),
      )
      let idx = 0
      for (const infos of results) {
        for (const info of infos) {
          if (info) {
            liveCollateral[idx] =
              rows[idx].side === 'short'
                ? info.lamports
                : info.data.length >= 72
                  ? Number(info.data.readBigUInt64LE(64)) // SPL/Token-2022 amount @ offset 64
                  : rows[idx].collateral_amount
          }
          idx++
        }
      }
    } catch {
      /* keep posted-collateral fallback */
    }
  }

  const positions: PositionWithKey[] = rows.map((row, i) => {
    const interest = projectAccruedInterest(
      row.debt_amount,
      row.accrued_interest_stored,
      row.last_update_slot,
      currentSlot,
    )
    const { totalOwed, debtValueSol, currentLtvBps, health } = computePositionHealth(
      row.side,
      liveCollateral[i],
      row.debt_amount,
      interest,
      solReserves,
      tokenReserves,
    )
    return {
      owner: row.owner,
      side: row.side,
      position_index: row.position_index,
      collateral_amount: liveCollateral[i],
      debt_amount: row.debt_amount,
      accrued_interest: interest,
      accrued_interest_stored: row.accrued_interest_stored,
      last_update_slot: row.last_update_slot,
      total_owed: totalOwed,
      debt_value_sol: debtValueSol,
      current_ltv_bps: currentLtvBps,
      health,
      owner_is_vault: row.owner_is_vault,
    }
  })

  const healthOrder: Record<string, number> = { liquidatable: 0, at_risk: 1, healthy: 2, none: 3 }
  positions.sort((a, b) => (healthOrder[a.health] ?? 3) - (healthOrder[b.health] ?? 3))

  return { positions, pool_price_sol: poolPriceSol }
}

// ============================================================================
// Vault Queries (V2.0)
// ============================================================================

/**
 * Get vault state by the vault creator's public key.
 *
 * Returns vault balance, authority, linked wallet count, etc.
 * Returns null if no vault exists for this creator.
 */
export const getVault = async (
  connection: Connection,
  creatorStr: string,
): Promise<VaultInfo | null> => {
  const creator = new PublicKey(creatorStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [vaultPda] = getTorchVaultPda(creator)
  const [vaultSolPda] = getVaultSolPda(creator)
  const [accountInfo, vaultSolInfo, rentExempt0] = await Promise.all([
    connection.getAccountInfo(vaultPda),
    connection.getAccountInfo(vaultSolPda),
    connection.getMinimumBalanceForRentExemption(0),
  ])

  if (!accountInfo) return null

  const vault = coder.accounts.decode('TorchVault', accountInfo.data) as unknown as TorchVault

  return {
    address: vaultPda.toString(),
    creator: vault.creator.toString(),
    authority: vault.authority.toString(),
    // [V21] Vault SOL lives in the System-owned torch_vault_sol PDA. Report the
    // program's OWN derived balance (lamports − rent floor, vault.rs
    // vault_physical_sol) — raw lamports made the UI's Max overshoot by the
    // rent-exempt minimum and every full withdrawal failed.
    sol_balance: Math.max(0, (vaultSolInfo?.lamports ?? 0) - rentExempt0) / LAMPORTS_PER_SOL,
    total_deposited: Number(vault.total_deposited.toString()) / LAMPORTS_PER_SOL,
    total_withdrawn: Number(vault.total_withdrawn.toString()) / LAMPORTS_PER_SOL,
    total_spent: Number(vault.total_spent.toString()) / LAMPORTS_PER_SOL,
    total_received: Number(vault.total_received.toString()) / LAMPORTS_PER_SOL,
    linked_wallets: vault.linked_wallets,
    created_at: Number(vault.created_at.toString()),
  }
}

/**
 * Get vault state by looking up a linked wallet's VaultWalletLink.
 *
 * Useful when you have an agent wallet and need to find its vault.
 * Returns null if the wallet is not linked to any vault.
 */
export const getVaultForWallet = async (
  connection: Connection,
  walletStr: string,
): Promise<VaultInfo | null> => {
  const wallet = new PublicKey(walletStr)
  const coder = new BorshCoder(idl as unknown as Idl)
  const [walletLinkPda] = getVaultWalletLinkPda(wallet)
  const linkInfo = await connection.getAccountInfo(walletLinkPda)
  if (!linkInfo) return null

  const link = coder.accounts.decode('VaultWalletLink', linkInfo.data) as unknown as VaultWalletLink

  // Now fetch the vault using the vault PDA stored in the link
  const vaultInfo = await connection.getAccountInfo(link.vault)
  if (!vaultInfo) return null

  const vault = coder.accounts.decode('TorchVault', vaultInfo.data) as unknown as TorchVault
  // [V21] Vault SOL lives in the System-owned torch_vault_sol PDA.
  const [vaultSolPda] = getVaultSolPda(vault.creator)
  const vaultSolInfo = await connection.getAccountInfo(vaultSolPda)

  return {
    address: link.vault.toString(),
    creator: vault.creator.toString(),
    authority: vault.authority.toString(),
    sol_balance: (vaultSolInfo?.lamports ?? 0) / LAMPORTS_PER_SOL,
    total_deposited: Number(vault.total_deposited.toString()) / LAMPORTS_PER_SOL,
    total_withdrawn: Number(vault.total_withdrawn.toString()) / LAMPORTS_PER_SOL,
    total_spent: Number(vault.total_spent.toString()) / LAMPORTS_PER_SOL,
    total_received: Number(vault.total_received.toString()) / LAMPORTS_PER_SOL,
    linked_wallets: vault.linked_wallets,
    created_at: Number(vault.created_at.toString()),
  }
}

/**
 * Get wallet link state for a specific wallet.
 *
 * Returns the link info (which vault it's linked to, when) or null if not linked.
 */
export const getVaultWalletLink = async (
  connection: Connection,
  walletStr: string,
): Promise<VaultWalletLinkInfo | null> => {
  const wallet = new PublicKey(walletStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [walletLinkPda] = getVaultWalletLinkPda(wallet)
  const accountInfo = await connection.getAccountInfo(walletLinkPda)

  if (!accountInfo) return null

  const link = coder.accounts.decode(
    'VaultWalletLink',
    accountInfo.data,
  ) as unknown as VaultWalletLink

  return {
    address: walletLinkPda.toString(),
    vault: link.vault.toString(),
    wallet: link.wallet.toString(),
    linked_at: Number(link.linked_at.toString()),
  }
}

// Per-user trading stats (volume, rewards claimed). Returns null if the user has
// no stats account yet (no trading activity).
export const getUserStats = async (
  connection: Connection,
  walletStr: string,
): Promise<UserStatsInfo | null> => {
  const wallet = new PublicKey(walletStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [userStatsPda] = getUserStatsPda(wallet)
  const accountInfo = await connection.getAccountInfo(userStatsPda)
  if (!accountInfo) return null

  const stats = coder.accounts.decode('UserStats', accountInfo.data) as unknown as UserStats

  return {
    address: userStatsPda.toString(),
    user: stats.user.toString(),
    total_volume_sol: Number(stats.total_volume.toString()) / LAMPORTS_PER_SOL,
    volume_current_epoch_sol: Number(stats.volume_current_epoch.toString()) / LAMPORTS_PER_SOL,
    volume_previous_epoch_sol: Number(stats.volume_previous_epoch.toString()) / LAMPORTS_PER_SOL,
    last_epoch_claimed: Number(stats.last_epoch_claimed.toString()),
    total_rewards_claimed_sol: Number(stats.total_rewards_claimed.toString()) / LAMPORTS_PER_SOL,
    last_volume_epoch: Number(stats.last_volume_epoch.toString()),
  }
}

// Protocol treasury state (current epoch, balances, distribution accounting).
// Returns null if the protocol hasn't been initialized yet.
export const getProtocolTreasuryState = async (
  connection: Connection,
): Promise<ProtocolTreasuryInfo | null> => {
  const coder = new BorshCoder(idl as unknown as Idl)

  const [protocolTreasuryPda] = getProtocolTreasuryPda()
  const accountInfo = await connection.getAccountInfo(protocolTreasuryPda)
  if (!accountInfo) return null

  const t = coder.accounts.decode(
    'ProtocolTreasury',
    accountInfo.data,
  ) as unknown as ProtocolTreasury

  return {
    address: protocolTreasuryPda.toString(),
    authority: t.authority.toString(),
    current_balance_sol: Number(t.current_balance.toString()) / LAMPORTS_PER_SOL,
    reserve_floor_sol: Number(t.reserve_floor.toString()) / LAMPORTS_PER_SOL,
    total_fees_received_sol: Number(t.total_fees_received.toString()) / LAMPORTS_PER_SOL,
    total_distributed_sol: Number(t.total_distributed.toString()) / LAMPORTS_PER_SOL,
    current_epoch: Number(t.current_epoch.toString()),
    last_epoch_ts: Number(t.last_epoch_ts.toString()),
    total_volume_current_epoch_sol:
      Number(t.total_volume_current_epoch.toString()) / LAMPORTS_PER_SOL,
    total_volume_previous_epoch_sol:
      Number(t.total_volume_previous_epoch.toString()) / LAMPORTS_PER_SOL,
    distributable_amount_sol: Number(t.distributable_amount.toString()) / LAMPORTS_PER_SOL,
  }
}

// Per-token Treasury state: SOL balance, tokens held, harvested fees, stars,
// and baseline pool reserves captured at migration. Returns null if the token
// or its treasury hasn't been created yet.
export const getTreasuryState = async (
  connection: Connection,
  mintStr: string,
): Promise<TreasuryInfo | null> => {
  const mint = new PublicKey(mintStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [treasurySolVaultPda] = getTreasurySolVaultPda(mint)
  const [accountInfo, vaultInfo] = await Promise.all([
    connection.getAccountInfo(treasuryPda),
    connection.getAccountInfo(treasurySolVaultPda),
  ])
  if (!accountInfo) return null

  const t = coder.accounts.decode('Treasury', accountInfo.data) as unknown as Treasury

  return {
    address: treasuryPda.toString(),
    bonding_curve: t.bonding_curve.toString(),
    mint: t.mint.toString(),
    treasury_sol_vault_sol: (vaultInfo?.lamports ?? 0) / LAMPORTS_PER_SOL,
    is_community_token: t.is_community_token,
    harvested_fees_sol: Number(t.harvested_fees.toString()) / LAMPORTS_PER_SOL,
    baseline_sol_reserves: Number(t.baseline_sol_reserves.toString()),
    baseline_token_reserves: Number(t.baseline_token_reserves.toString()),
    baseline_initialized: t.baseline_initialized,
    short_selling_enabled: t.short_selling_enabled,
    lending_enabled: t.lending_enabled,
    last_buyback_slot: Number(t.last_buyback_slot.toString()),
    total_tokens_lent: Number(t.total_tokens_lent.toString()),
    active_shorts: Number(t.active_shorts.toString()),
    total_sol_lent_to_longs: Number(t.total_sol_lent_to_longs.toString()),
    active_longs: Number(t.active_longs.toString()),
    total_token_collateral_locked: Number(t.total_token_collateral_locked.toString()),
  }
}

// Re-export for internal use by other SDK modules
export { fetchTokenRaw }
