import { Connection, PublicKey, ParsedTransactionWithMeta } from '@solana/web3.js'
import { getBondingCurvePda, getDeepPoolAccounts, getTrades, getSwaps } from 'torchsdk'
import {
  LAMPORTS_PER_SOL,
  TOKEN_MULTIPLIER,
  initialVirtualReserves,
} from './constants'

const isDev = process.env.NODE_ENV === 'development'

export interface PricePoint {
  timestamp: number
  price: number // in SOL per token
  volume: number // in SOL
  isBuy: boolean
  trader: string // wallet address of the trader
}

/** Compute the bonding-curve spot price given the real SOL reserves in lamports.
 *  Uses per-tier initial virtual reserves based on bonding target. */
function bondingCurvePrice(realSolLamports: number, bondingTargetLamports: bigint): number {
  const { ivs, ivt } = initialVirtualReserves(bondingTargetLamports)
  const initialVirtualSol = Number(ivs)
  const initialVirtualTokens = Number(ivt)
  const virtualSol = initialVirtualSol + realSolLamports
  const tokensRemoved =
    (initialVirtualTokens * realSolLamports) / (initialVirtualSol + realSolLamports)
  const virtualTokens = initialVirtualTokens - tokensRemoved
  return ((virtualSol / virtualTokens) * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
}

/**
 * Fetch real price history by replaying SOL balance changes on the bonding
 * curve PDA.  We walk transactions from newest → oldest, using the known
 * current `solRaised` to reconstruct the SOL level (and therefore the price)
 * at every historical trade.
 */
export async function fetchPriceHistory(
  connection: Connection,
  mintAddress: string,
  currentSolRaised: number,
  bondingTargetSol: number = 200,
): Promise<PricePoint[]> {
  const bondingTargetLamports = BigInt(Math.round(bondingTargetSol * LAMPORTS_PER_SOL))
  const mint = new PublicKey(mintAddress)
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const bcAddress = bondingCurvePda.toString()

  const signatures = await connection.getSignaturesForAddress(
    bondingCurvePda,
    { limit: 200 },
    'confirmed',
  )

  if (signatures.length === 0) return []

  // Batch-fetch parsed transactions
  const BATCH_SIZE = 100
  const allTxs: (ParsedTransactionWithMeta | null)[] = []

  for (let i = 0; i < signatures.length; i += BATCH_SIZE) {
    const batch = signatures.slice(i, i + BATCH_SIZE)
    try {
      const txs = await connection.getParsedTransactions(
        batch.map((s) => s.signature),
        { maxSupportedTransactionVersion: 0 },
      )
      allTxs.push(...txs)
    } catch {
      allTxs.push(...new Array(batch.length).fill(null))
    }
  }

  // Walk newest → oldest, reconstructing historical SOL reserves
  let runningRealSol = currentSolRaised * LAMPORTS_PER_SOL
  const points: PricePoint[] = []

  for (let i = 0; i < allTxs.length; i++) {
    const tx = allTxs[i]
    const sig = signatures[i]
    if (!tx?.meta || tx.meta.err) continue

    const accountKeys = tx.transaction.message.accountKeys
    const bcIndex = accountKeys.findIndex((k) => k.pubkey.toString() === bcAddress)
    if (bcIndex === -1) continue

    const solChange = tx.meta.postBalances[bcIndex] - tx.meta.preBalances[bcIndex]
    if (solChange === 0) continue // not a trade

    // Skip the migration tx — it drains the curve's SOL into the DeepPool
    // and would otherwise render as a single sell-to-floor candle.
    const isMigration = tx.meta.logMessages?.some(
      (l) =>
        l.includes('Instruction: MigrateToDex') ||
        l.includes('Instruction: FundMigrationWsol'),
    )
    if (isMigration) {
      // Still rewind runningRealSol so older trades are priced correctly.
      runningRealSol -= solChange
      continue
    }

    const isBuy = solChange > 0
    const volume = Math.abs(solChange) / LAMPORTS_PER_SOL
    const trader = accountKeys[0]?.pubkey?.toString() || ''

    // Price AFTER this trade (current runningRealSol)
    const price = bondingCurvePrice(Math.max(0, runningRealSol), bondingTargetLamports)

    points.push({
      timestamp: sig.blockTime || 0,
      price,
      volume,
      isBuy,
      trader,
    })

    // Step backwards: undo this trade's effect
    runningRealSol -= solChange
  }

  // Chronological order
  points.reverse()
  return points
}

/**
 * Fetch price history from DeepPool pool transactions for a migrated token.
 *
 * DeepPool stores SOL directly on the pool PDA's lamports and tokens in a
 * Token-2022 vault. We walk the pool's signatures and read:
 *   - SOL side  → pool PDA lamports change in postBalances
 *   - Token side → token-vault postTokenBalance
 *
 * Price = (poolLamports / tokenVaultAmount) normalized by decimals.
 */
export async function fetchDeepPoolPriceHistory(
  connection: Connection,
  mint: PublicKey,
): Promise<PricePoint[]> {
  const deepPool = getDeepPoolAccounts(mint)
  const poolAddress = deepPool.pool.toString()
  const tokenVaultAddress = deepPool.tokenVault.toString()

  const signatures = await connection.getSignaturesForAddress(
    deepPool.pool,
    { limit: 200 },
    'confirmed',
  )

  if (signatures.length === 0) return []

  // Batch-fetch parsed transactions
  const BATCH_SIZE = 100
  const allTxs: (ParsedTransactionWithMeta | null)[] = []

  for (let i = 0; i < signatures.length; i += BATCH_SIZE) {
    const batch = signatures.slice(i, i + BATCH_SIZE)
    try {
      const txs = await connection.getParsedTransactions(
        batch.map((s) => s.signature),
        { maxSupportedTransactionVersion: 0 },
      )
      allTxs.push(...txs)
    } catch {
      allTxs.push(...new Array(batch.length).fill(null))
    }
  }

  const points: PricePoint[] = []

  for (let i = 0; i < allTxs.length; i++) {
    const tx = allTxs[i]
    const sig = signatures[i]
    if (!tx?.meta || tx.meta.err) continue

    const accountKeys = tx.transaction.message.accountKeys
    const poolIndex = accountKeys.findIndex((k) => k.pubkey.toString() === poolAddress)
    if (poolIndex === -1) continue

    const postPoolLamports = tx.meta.postBalances[poolIndex]
    const prePoolLamports = tx.meta.preBalances[poolIndex]
    const solChange = postPoolLamports - prePoolLamports

    let postTokenVaultBalance: number | null = null
    for (const bal of tx.meta.postTokenBalances || []) {
      const accountAddress = accountKeys[bal.accountIndex]?.pubkey?.toString()
      if (accountAddress === tokenVaultAddress) {
        postTokenVaultBalance = Number(bal.uiTokenAmount.amount)
        break
      }
    }

    if (postTokenVaultBalance === null || postTokenVaultBalance === 0) continue
    if (solChange === 0) continue // not a swap

    // Pool's lamports include rent — we approximate rent as constant across txs,
    // so the difference cancels when reading just the changes. For the spot
    // price we use the raw lamports value (rent floor is small vs. liquidity).
    const price = (postPoolLamports / LAMPORTS_PER_SOL) / (postTokenVaultBalance / TOKEN_MULTIPLIER)
    const volume = Math.abs(solChange) / LAMPORTS_PER_SOL
    const isBuy = solChange > 0
    const trader = accountKeys[0]?.pubkey?.toString() || ''

    points.push({
      timestamp: sig.blockTime || 0,
      price,
      volume,
      isBuy,
      trader,
    })
  }

  // Already in newest-first order from signatures; reverse to chronological
  points.reverse()

  // Skip the first (oldest) transaction — it's the migration liquidity deposit,
  // not a real trade (appears as a large baseline-funding event).
  if (points.length > 0) {
    points.shift()
  }

  return points
}

/**
 * Map an indexer BondingCurveTrade row to the in-app PricePoint shape.
 * Price is derived from virtual reserves *after* the trade — the post-state
 * marginal price, same convention as the candle aggregation.
 */
function indexerTradeToPricePoint(r: {
  created_at: string
  is_buy: boolean
  sol_in: number
  sol_out: number
  trader: string
  virtual_sol_after: number
  virtual_token_after: number
}): PricePoint {
  const timestamp = Math.floor(new Date(r.created_at).getTime() / 1000)
  const volume = (r.sol_in + r.sol_out) / LAMPORTS_PER_SOL
  const price =
    r.virtual_token_after > 0
      ? (r.virtual_sol_after / r.virtual_token_after) * TOKEN_MULTIPLIER / LAMPORTS_PER_SOL
      : 0
  return { timestamp, price, volume, isBuy: r.is_buy, trader: r.trader }
}

/** Map an indexer DEX SwapExecuted row to PricePoint. Post-trade marginal
 *  price from `sol_reserve_after / token_reserve_after`. Volume is the
 *  user-facing SOL leg of the trade: amount_in for buys, amount_out for sells. */
function indexerSwapToPricePoint(r: {
  created_at: string
  is_buy: boolean
  user_pk: string
  amount_in_net: number
  amount_out_net: number
  sol_reserve_after: number
  token_reserve_after: number
}): PricePoint {
  const timestamp = Math.floor(new Date(r.created_at).getTime() / 1000)
  const volumeLamports = r.is_buy ? r.amount_in_net : r.amount_out_net
  const price =
    r.token_reserve_after > 0
      ? (r.sol_reserve_after / r.token_reserve_after) * TOKEN_MULTIPLIER / LAMPORTS_PER_SOL
      : 0
  return {
    timestamp,
    price,
    volume: volumeLamports / LAMPORTS_PER_SOL,
    isBuy: r.is_buy,
    trader: r.user_pk,
  }
}

/**
 * Fetch combined price history: bonding curve trades + DeepPool trades.
 * Returns a unified PricePoint[] with bonding history first, then DeepPool.
 *
 * Indexer-first when `indexerUrl` is provided — single HTTP fetch per side
 * vs. 200-signature RPC walks. Falls back to RPC scanning when no indexer
 * is configured or the indexer call errors.
 */
export async function fetchCombinedPriceHistory(
  connection: Connection,
  mintAddress: string,
  currentSolRaised: number,
  bondingTargetSol: number,
  indexerUrl?: string,
): Promise<PricePoint[]> {
  if (indexerUrl) {
    try {
      const [tradeRows, swapRows] = await Promise.all([
        getTrades({ indexer: indexerUrl, mint: mintAddress, limit: 200 }),
        getSwaps({ indexer: indexerUrl, tokenMint: mintAddress, limit: 200 }),
      ])
      // Indexer returns newest-first; the chart/trades-list consumers
      // accept either order (chart sorts by time, trades view reverses).
      // We chronologically sort so bonding precedes DEX naturally.
      const bonding = tradeRows.map(indexerTradeToPricePoint)
      const dex = swapRows.map(indexerSwapToPricePoint)
      return [...bonding, ...dex].sort((a, b) => a.timestamp - b.timestamp)
    } catch (err) {
      if (isDev) console.error('Indexer trade fetch failed, falling back to RPC:', err)
    }
  }

  const mint = new PublicKey(mintAddress)
  const [bondingHistory, deepPoolHistory] = await Promise.all([
    fetchPriceHistory(connection, mintAddress, currentSolRaised, bondingTargetSol).catch((err) => {
      if (isDev) console.error('Failed to fetch bonding history:', err)
      return [] as PricePoint[]
    }),
    fetchDeepPoolPriceHistory(connection, mint).catch((err) => {
      if (isDev) console.error('Failed to fetch DeepPool history:', err)
      return [] as PricePoint[]
    }),
  ])

  return [...bondingHistory, ...deepPoolHistory]
}
