import { Connection, PublicKey, ParsedTransactionWithMeta } from '@solana/web3.js'
import { getBondingCurvePda, getDeepPoolAccounts } from 'torchsdk'
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
 * Fetch combined price history: bonding curve trades + DeepPool trades.
 * Returns a unified PricePoint[] with bonding history first, then DeepPool.
 */
export async function fetchCombinedPriceHistory(
  connection: Connection,
  mintAddress: string,
  currentSolRaised: number,
  bondingTargetSol: number,
): Promise<PricePoint[]> {
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
