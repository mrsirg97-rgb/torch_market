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
  LoanPosition,
  ShortPosition,
  UserStats,
  ProtocolTreasury,
  getBondingCurvePda,
  getTokenTreasuryPda,
  getLoanPositionPda,
  getShortPositionPda,
  getCollateralVaultPda,
  getTorchVaultPda,
  getVaultWalletLinkPda,
  getUserStatsPda,
  getProtocolTreasuryPda,
  getDeepPoolAccounts,
  calculateBondingProgress,
  calculatePrice,
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
  LoanPositionInfo,
  ShortPositionInfo,
  LoanPositionWithKey,
  AllLoanPositionsResult,
  VaultInfo,
  VaultWalletLinkInfo,
  UserStatsInfo,
  ProtocolTreasuryInfo,
  TreasuryInfo,
  TokenMetadataResult,
} from './types'

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
const fetchMintsMetadata = async (
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

  const treasurySol = treasury ? Number(treasury.sol_balance.toString()) / LAMPORTS_PER_SOL : 0
  const stars = treasury ? Number(treasury.total_stars.toString()) : 0

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
 */
export const getTokens = async (
  connection: Connection,
  params: TokenListParams = {},
): Promise<TokenListResult> => {
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
  opts?: { source?: 'bonding' | 'pool' | 'all'; enrich?: boolean },
): Promise<MessagesResult> => {
  const mint = new PublicKey(mintStr)
  const safeLimit = Math.min(limit, 100)
  const sigLimit = Math.min(safeLimit, 50)
  const source = opts?.source ?? 'all'

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
const INTEREST_RATE_BPS = 200 // 2% per epoch
const LIQUIDATION_THRESHOLD_BPS = 6500 // 65%
const LIQUIDATION_BONUS_BPS = 1000 // 10%
const LENDING_UTILIZATION_CAP_BPS = 8000 // 80% (V4.0, was 70%)
const BORROW_SHARE_MULTIPLIER = 23 // Per-user cap: max borrow = 23x collateral share of supply (V10.2.5, was 5x)
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

// Depth-based risk bands (V7): pool SOL depth → max LTV
const MIN_POOL_SOL_LENDING = 5_000_000_000 // 5 SOL
const DEPTH_TIER_1 = 50_000_000_000 // 50 SOL
const DEPTH_TIER_2 = 200_000_000_000 // 200 SOL
const DEPTH_TIER_3 = 500_000_000_000 // 500 SOL
const DEPTH_LTV_0 = 2500 // <50 SOL  → 25%
const DEPTH_LTV_1 = 3500 // 50-200   → 35%
const DEPTH_LTV_2 = 4500 // 200-500  → 45%
const DEPTH_LTV_3 = 5000 // 500+     → 50%

const getDepthMaxLtvBps = (poolSol: number): number => {
  if (poolSol < MIN_POOL_SOL_LENDING) return 0
  if (poolSol < DEPTH_TIER_1) return DEPTH_LTV_0
  if (poolSol < DEPTH_TIER_2) return DEPTH_LTV_1
  if (poolSol < DEPTH_TIER_3) return DEPTH_LTV_2
  return DEPTH_LTV_3
}

/**
 * Get lending info for a migrated token.
 *
 * Returns interest rates, LTV limits, and active loan statistics.
 * Lending is available on all migrated tokens with treasury SOL.
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

  const treasurySol = treasury ? Number(treasury.sol_balance.toString()) : 0

  // Fetch pool SOL depth for depth-band max LTV
  let poolSol = 0
  try {
    const reserves = await fetchDeepPoolReserves(connection, mint)
    poolSol = reserves.solReserves
  } catch {
    // Fall back to minimum tier if pool fetch fails
  }
  const effectiveMaxLtv = getDepthMaxLtvBps(poolSol)

  // Scan for active loan positions via collateral vault balance
  const [collateralVaultPda] = getCollateralVaultPda(mint)
  const vaultInfo = await connection.getAccountInfo(collateralVaultPda)

  // Count active loans by scanning LoanPosition accounts
  let activeLoans: number | null = 0
  let totalSolLent: number | null = 0
  const warnings: string[] = []

  try {
    // Derive discriminator from IDL rather than hardcoding
    const coder = new BorshCoder(idl as Idl)
    const loanDiscriminator = coder.accounts.accountDiscriminator('LoanPosition')
    const bs58 = await import('bs58')
    const accounts = await connection.getProgramAccounts(PROGRAM_ID, {
      filters: [
        { memcmp: { offset: 0, bytes: bs58.default.encode(loanDiscriminator) } },
        { memcmp: { offset: 8 + 32, bytes: mint.toBase58() } }, // mint at offset 40
      ],
      dataSlice: { offset: 8 + 32 + 32, length: 16 }, // collateral_amount + borrowed_amount
    })

    for (const acc of accounts) {
      try {
        // Read borrowed_amount (u64 at offset 8 within the slice)
        const borrowed = acc.account.data.readBigUInt64LE(8)
        if (borrowed > BigInt(0)) {
          activeLoans = (activeLoans ?? 0) + 1
          totalSolLent = (totalSolLent ?? 0) + Number(borrowed)
        }
      } catch {
        // Skip malformed accounts
      }
    }
  } catch (e) {
    activeLoans = null
    totalSolLent = null
    warnings.push(`Loan enumeration failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  return {
    interest_rate_bps: INTEREST_RATE_BPS,
    max_ltv_bps: effectiveMaxLtv,
    liquidation_threshold_bps: LIQUIDATION_THRESHOLD_BPS,
    liquidation_bonus_bps: LIQUIDATION_BONUS_BPS,
    utilization_cap_bps: LENDING_UTILIZATION_CAP_BPS,
    borrow_share_multiplier: BORROW_SHARE_MULTIPLIER,
    total_sol_lent: totalSolLent,
    active_loans: activeLoans,
    treasury_sol_available: Math.max(
      0,
      Math.floor((treasurySol * LENDING_UTILIZATION_CAP_BPS) / 10000) - (totalSolLent ?? 0),
    ),
    ...(warnings.length > 0 ? { warnings } : {}),
  }
}

/**
 * Get loan position for a wallet on a specific token.
 *
 * Returns collateral locked, SOL owed, health status, etc.
 * Returns health="none" if no active loan exists.
 */
export const getLoanPosition = async (
  connection: Connection,
  mintStr: string,
  walletStr: string,
): Promise<LoanPositionInfo> => {
  const mint = new PublicKey(mintStr)
  const wallet = new PublicKey(walletStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [loanPositionPda] = getLoanPositionPda(mint, wallet)
  const [accountInfo, currentSlot] = await Promise.all([
    connection.getAccountInfo(loanPositionPda),
    connection.getSlot('confirmed'),
  ])

  if (!accountInfo) {
    return {
      collateral_amount: 0,
      borrowed_amount: 0,
      accrued_interest: 0,
      accrued_interest_stored: 0,
      last_update_slot: 0,
      total_owed: 0,
      collateral_value_sol: 0,
      current_ltv_bps: 0,
      health: 'none',
    }
  }

  const loan = coder.accounts.decode('LoanPosition', accountInfo.data) as unknown as LoanPosition

  const collateral = Number(loan.collateral_amount.toString())
  const borrowed = Number(loan.borrowed_amount.toString())
  const storedInterest = Number(loan.accrued_interest.toString())
  const lastUpdateSlot = Number(loan.last_update_slot.toString())
  const interest = projectAccruedInterest(borrowed, storedInterest, lastUpdateSlot, currentSlot)
  const totalOwed = borrowed + interest

  // Get collateral value from DeepPool price
  let collateralValueSol: number | null = 0
  let poolSol = 0
  const warnings: string[] = []
  try {
    const reserves = await fetchDeepPoolReserves(connection, mint)
    poolSol = reserves.solReserves
    if (reserves.tokenReserves > 0) {
      collateralValueSol = (collateral * poolSol) / reserves.tokenReserves
    }
  } catch (e) {
    collateralValueSol = null
    warnings.push(`Collateral valuation failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  let currentLtvBps: number | null
  if (collateralValueSol === null) {
    currentLtvBps = null
  } else if (collateralValueSol > 0) {
    currentLtvBps = Math.floor((totalOwed / collateralValueSol) * 10000)
  } else {
    currentLtvBps = totalOwed > 0 ? 10000 : 0
  }

  const maxLtvBps = getDepthMaxLtvBps(poolSol)
  let health: LoanPositionInfo['health']
  if (borrowed === 0 && interest === 0) {
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

  return {
    collateral_amount: collateral,
    borrowed_amount: borrowed,
    accrued_interest: interest,
    accrued_interest_stored: storedInterest,
    last_update_slot: lastUpdateSlot,
    total_owed: totalOwed,
    collateral_value_sol: collateralValueSol,
    current_ltv_bps: currentLtvBps,
    health,
    ...(warnings.length > 0 ? { warnings } : {}),
  }
}

/**
 * Get a user's short position for a given token.
 *
 * Reads the ShortPosition PDA on-chain and computes health status
 * using the DeepPool price to value the token debt against SOL collateral.
 */
export const getShortPosition = async (
  connection: Connection,
  mintStr: string,
  walletStr: string,
): Promise<ShortPositionInfo> => {
  const mint = new PublicKey(mintStr)
  const wallet = new PublicKey(walletStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  const [shortPositionPda] = getShortPositionPda(mint, wallet)
  const [accountInfo, currentSlot] = await Promise.all([
    connection.getAccountInfo(shortPositionPda),
    connection.getSlot('confirmed'),
  ])

  if (!accountInfo) {
    return {
      sol_collateral: 0,
      tokens_borrowed: 0,
      accrued_interest: 0,
      accrued_interest_stored: 0,
      last_update_slot: 0,
      total_owed_tokens: 0,
      debt_value_sol: 0,
      current_ltv_bps: 0,
      health: 'none',
    }
  }

  const short = coder.accounts.decode('ShortPosition', accountInfo.data) as unknown as ShortPosition

  const solCollateral = Number(short.sol_collateral.toString())
  const tokensBorrowed = Number(short.tokens_borrowed.toString())
  const storedInterest = Number(short.accrued_interest.toString())
  const lastUpdateSlot = Number(short.last_update_slot.toString())
  const interest = projectAccruedInterest(
    tokensBorrowed,
    storedInterest,
    lastUpdateSlot,
    currentSlot,
  )
  const totalOwedTokens = tokensBorrowed + interest

  // Get token debt value from DeepPool price
  let debtValueSol: number | null = 0
  let poolSol = 0
  const warnings: string[] = []
  try {
    const reserves = await fetchDeepPoolReserves(connection, mint)
    poolSol = reserves.solReserves
    if (reserves.tokenReserves > 0) {
      debtValueSol = (totalOwedTokens * poolSol) / reserves.tokenReserves
    }
  } catch (e) {
    debtValueSol = null
    warnings.push(`Debt valuation failed: ${e instanceof Error ? e.message : String(e)}`)
  }

  // For shorts, LTV = debt_value_sol / sol_collateral
  let currentLtvBps: number | null
  if (debtValueSol === null) {
    currentLtvBps = null
  } else if (solCollateral > 0) {
    currentLtvBps = Math.floor((debtValueSol / solCollateral) * 10000)
  } else {
    currentLtvBps = totalOwedTokens > 0 ? 10000 : 0
  }

  const maxLtvBps = getDepthMaxLtvBps(poolSol)
  let health: ShortPositionInfo['health']
  if (tokensBorrowed === 0 && interest === 0) {
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

  return {
    sol_collateral: solCollateral,
    tokens_borrowed: tokensBorrowed,
    accrued_interest: interest,
    accrued_interest_stored: storedInterest,
    last_update_slot: lastUpdateSlot,
    total_owed_tokens: totalOwedTokens,
    debt_value_sol: debtValueSol,
    current_ltv_bps: currentLtvBps,
    health,
    ...(warnings.length > 0 ? { warnings } : {}),
  }
}

/**
 * Get all active loan positions for a given token mint.
 *
 * Scans on-chain LoanPosition accounts, computes health for each,
 * and returns them sorted: liquidatable first, then at_risk, then healthy.
 */
export const getAllLoanPositions = async (
  connection: Connection,
  mintStr: string,
): Promise<AllLoanPositionsResult> => {
  const mint = new PublicKey(mintStr)
  const coder = new BorshCoder(idl as unknown as Idl)

  // 1. Fetch all LoanPosition accounts for this mint
  const loanDiscriminator = coder.accounts.accountDiscriminator('LoanPosition')
  const bs58 = await import('bs58')
  const accounts = await connection.getProgramAccounts(PROGRAM_ID, {
    filters: [
      { memcmp: { offset: 0, bytes: bs58.default.encode(loanDiscriminator) } },
      { memcmp: { offset: 8 + 32, bytes: mint.toBase58() } }, // mint at offset 40
    ],
  })

  // 2. Decode and filter to active loans (borrowed_amount > 0)
  const activeLoans: { borrower: string; loan: LoanPosition }[] = []
  for (const acc of accounts) {
    try {
      const loan = coder.accounts.decode(
        'LoanPosition',
        acc.account.data,
      ) as unknown as LoanPosition
      const borrowed = Number(loan.borrowed_amount.toString())
      if (borrowed > 0) {
        activeLoans.push({
          borrower: loan.user.toString(),
          loan,
        })
      }
    } catch {
      // Skip malformed accounts
    }
  }

  // 3. Fetch DeepPool reserves + current slot ONCE (interest projection needs currentSlot)
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
    // Pool price unavailable — fall back to a plain slot fetch so we can still project interest
    try {
      currentSlot = await connection.getSlot('confirmed')
    } catch {
      /* ignore */
    }
  }

  // 4. Compute health for each position (interest projected to currentSlot)
  const positions: LoanPositionWithKey[] = activeLoans.map(({ borrower, loan }) => {
    const collateral = Number(loan.collateral_amount.toString())
    const borrowed = Number(loan.borrowed_amount.toString())
    const storedInterest = Number(loan.accrued_interest.toString())
    const lastUpdateSlot = Number(loan.last_update_slot.toString())
    const interest = projectAccruedInterest(borrowed, storedInterest, lastUpdateSlot, currentSlot)
    const totalOwed = borrowed + interest

    let collateralValueSol: number | null = null
    if (poolPriceSol !== null && tokenReserves > 0) {
      collateralValueSol = (collateral * solReserves) / tokenReserves
    }

    let currentLtvBps: number | null
    if (collateralValueSol === null) {
      currentLtvBps = null
    } else if (collateralValueSol > 0) {
      currentLtvBps = Math.floor((totalOwed / collateralValueSol) * 10000)
    } else {
      currentLtvBps = totalOwed > 0 ? 10000 : 0
    }

    const maxLtvBps = getDepthMaxLtvBps(solReserves)
    let health: LoanPositionInfo['health']
    if (borrowed === 0 && interest === 0) {
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

    return {
      borrower,
      collateral_amount: collateral,
      borrowed_amount: borrowed,
      accrued_interest: interest,
      accrued_interest_stored: storedInterest,
      last_update_slot: lastUpdateSlot,
      total_owed: totalOwed,
      collateral_value_sol: collateralValueSol,
      current_ltv_bps: currentLtvBps,
      health,
    }
  })

  // 5. Sort: liquidatable first, then at_risk, then healthy
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
  const accountInfo = await connection.getAccountInfo(vaultPda)

  if (!accountInfo) return null

  const vault = coder.accounts.decode('TorchVault', accountInfo.data) as unknown as TorchVault

  return {
    address: vaultPda.toString(),
    creator: vault.creator.toString(),
    authority: vault.authority.toString(),
    sol_balance: Number(vault.sol_balance.toString()) / LAMPORTS_PER_SOL,
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

  return {
    address: link.vault.toString(),
    creator: vault.creator.toString(),
    authority: vault.authority.toString(),
    sol_balance: Number(vault.sol_balance.toString()) / LAMPORTS_PER_SOL,
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
  const accountInfo = await connection.getAccountInfo(treasuryPda)
  if (!accountInfo) return null

  const t = coder.accounts.decode('Treasury', accountInfo.data) as unknown as Treasury

  return {
    address: treasuryPda.toString(),
    bonding_curve: t.bonding_curve.toString(),
    mint: t.mint.toString(),
    sol_balance_sol: Number(t.sol_balance.toString()) / LAMPORTS_PER_SOL,
    is_community_token: t.is_community_token,
    short_collateral_reserved: Number(t.short_collateral_reserved.toString()),
    harvested_fees_sol: Number(t.harvested_fees.toString()) / LAMPORTS_PER_SOL,
    baseline_sol_reserves: Number(t.baseline_sol_reserves.toString()),
    baseline_token_reserves: Number(t.baseline_token_reserves.toString()),
    baseline_initialized: t.baseline_initialized,
    short_selling_enabled: t.short_selling_enabled,
    last_buyback_slot: Number(t.last_buyback_slot.toString()),
    total_stars: Number(t.total_stars.toString()),
    star_sol_balance_sol: Number(t.star_sol_balance.toString()) / LAMPORTS_PER_SOL,
    creator_paid_out: t.creator_paid_out,
  }
}

// Re-export for internal use by other SDK modules
export { fetchTokenRaw }
