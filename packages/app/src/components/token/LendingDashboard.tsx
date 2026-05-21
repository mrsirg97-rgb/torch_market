'use client'

import { useState, useEffect, useCallback } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import {
  getLendingInfo,
  getLoanPosition,
  getAllLoanPositions,
  getShortPosition,
  getAllShortPositions,
  buildBorrowTransaction,
  buildRepayTransaction,
  buildLiquidateTransaction,
  buildLiquidateShortTransaction,
  buildOpenShortTransaction,
  buildCloseShortTransaction,
  getVault,
} from 'torchsdk'
import type {
  LendingInfo,
  LoanPositionInfo,
  AllLoanPositionsResult,
  ShortPositionInfo,
  AllShortPositionsResult,
  VaultInfo,
} from 'torchsdk'
import { LAMPORTS_PER_SOL, TOKEN_MULTIPLIER } from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'

/** Extract a human-readable error from transaction/simulation failures */
function parseLendingError(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  if (msg.includes('User rejected')) return 'Transaction cancelled.'
  // Anchor program errors: "Error Message: ..."
  const anchorMatch = msg.match(/Error Message:\s*(.+?)(?:\.|$)/)
  if (anchorMatch) return anchorMatch[1].trim()
  // Anchor error code: "AnchorError ... Message: ..."
  const anchorCodeMatch = msg.match(/AnchorError[^:]*:\s*(.+?)(?:\.|$)/)
  if (anchorCodeMatch) return anchorCodeMatch[1].trim()
  // Simulation failed with a program error
  const simMatch = msg.match(/Program \w+ failed:\s*(.+?)(?:\.|$)/)
  if (simMatch) return `Simulation failed: ${simMatch[1].trim()}`
  // Custom program error code
  const customMatch = msg.match(/custom program error:\s*(0x[0-9a-fA-F]+)/)
  if (customMatch) return `Program error: ${customMatch[1]}`
  // Instruction error
  const instrMatch = msg.match(/Error processing Instruction \d+:\s*(.+?)(?:\.|$)/)
  if (instrMatch) return instrMatch[1].trim()
  // Insufficient funds
  if (msg.includes('insufficient') || msg.includes('Insufficient')) return 'Insufficient funds'
  // Fallback: show first 120 chars instead of hiding
  if (msg.length > 120) return msg.slice(0, 120) + '...'
  return msg
}

interface LendingDashboardProps {
  mintAddress: string
  isMigrated: boolean
  symbol: string
  userTokenBalance: bigint
  /** Vault's balance of this mint, if the connected wallet has a vault. */
  vaultTokenBalance?: bigint | null
  priceInSol: number
  treasurySolBalance: number
  utilizationCapBps: number
  totalSolLent: number
}

// Confirm transaction with graceful timeout handling (same pattern as SwapPanel)
async function confirmTransactionSafe(
  connection: ReturnType<typeof useConnection>['connection'],
  signature: string,
  blockhash: string,
  lastValidBlockHeight: number,
): Promise<void> {
  try {
    const confirmation = await connection.confirmTransaction(
      { signature, blockhash, lastValidBlockHeight },
      'confirmed',
    )
    if (confirmation.value.err) {
      // Try to fetch logs for a more useful error message
      try {
        const tx = await connection.getTransaction(signature, {
          commitment: 'confirmed',
          maxSupportedTransactionVersion: 0,
        })
        if (tx?.meta?.logMessages) {
          const errorLog = tx.meta.logMessages.find(
            (log) => log.includes('Error Message:') || log.includes('AnchorError'),
          )
          if (errorLog) {
            // Extract just the message part, e.g. "Error Message: Loan-to-value ratio exceeds maximum"
            const msgMatch = errorLog.match(/Error Message:\s*(.+)/)
            if (msgMatch) throw new Error(msgMatch[1])
            throw new Error(errorLog)
          }
        }
      } catch (logErr) {
        if (logErr instanceof Error && logErr.message !== 'Transaction failed on-chain') {
          throw logErr
        }
      }
      throw new Error('Transaction failed on-chain')
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    const errName = err instanceof Error ? err.name : ''
    if (
      message.includes('block height exceeded') ||
      message.includes('TransactionExpiredBlockheightExceededError') ||
      errName.includes('TransactionExpired')
    ) {
      return
    }
    throw err
  }
}

function formatSolValue(lamports: number): string {
  const value = lamports / LAMPORTS_PER_SOL
  return value.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  })
}

function formatTokenValue(baseUnits: number): string {
  const value = baseUnits / TOKEN_MULTIPLIER
  if (value >= 1_000_000_000) return (value / 1_000_000_000).toFixed(2) + 'B'
  if (value >= 1_000_000) return (value / 1_000_000).toFixed(2) + 'M'
  if (value >= 1_000) return (value / 1_000).toFixed(2) + 'K'
  return value.toFixed(2)
}

function LtvBar({
  currentLtvBps,
  maxLtvBps,
  liquidationThresholdBps,
}: {
  currentLtvBps: number
  maxLtvBps: number
  liquidationThresholdBps: number
}) {
  // Scale: 0 to liquidationThresholdBps + 10% buffer for visual
  const scaleMax = liquidationThresholdBps * 1.15
  const fillPct = Math.min((currentLtvBps / scaleMax) * 100, 100)
  const maxLtvPct = (maxLtvBps / scaleMax) * 100
  const liqPct = (liquidationThresholdBps / scaleMax) * 100

  const fillColor =
    currentLtvBps >= liquidationThresholdBps
      ? 'bg-red-500'
      : currentLtvBps >= maxLtvBps
        ? 'bg-orange-500'
        : currentLtvBps >= maxLtvBps * 0.75
          ? 'bg-yellow-500'
          : 'bg-green-500'

  return (
    <div className="space-y-1">
      <div className="relative h-2 bg-white/10 rounded-full overflow-hidden">
        {/* Fill */}
        <div
          className={`absolute inset-y-0 left-0 rounded-full transition-all ${fillColor}`}
          style={{ width: `${fillPct}%` }}
        />
        {/* Max LTV marker */}
        <div
          className="absolute top-0 bottom-0 w-0.5 bg-yellow-400"
          style={{ left: `${maxLtvPct}%` }}
          title={`Max LTV: ${(maxLtvBps / 100).toFixed(0)}%`}
        />
        {/* Liquidation threshold marker */}
        <div
          className="absolute top-0 bottom-0 w-0.5 bg-red-500"
          style={{ left: `${liqPct}%` }}
          title={`Liquidation: ${(liquidationThresholdBps / 100).toFixed(0)}%`}
        />
      </div>
      <div className="flex justify-between text-[10px] text-white/30">
        <span>0%</span>
        <span className="text-accent/70">{(maxLtvBps / 100).toFixed(0)}% Max</span>
        <span className="text-danger/70">{(liquidationThresholdBps / 100).toFixed(0)}% Liq</span>
      </div>
    </div>
  )
}

export function LendingDashboard({
  mintAddress,
  isMigrated,
  symbol,
  userTokenBalance,
  vaultTokenBalance,
  priceInSol,
  treasurySolBalance,
  utilizationCapBps,
  totalSolLent,
}: LendingDashboardProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const [lendingInfo, setLendingInfo] = useState<LendingInfo | null>(null)
  const [loanPosition, setLoanPosition] = useState<LoanPositionInfo | null>(null)
  const [lendingLoading, setLendingLoading] = useState(false)
  const [loanLoading, setLoanLoading] = useState(false)

  // All loan positions state
  const [allPositions, setAllPositions] = useState<AllLoanPositionsResult | null>(null)
  const [positionsLoading, setPositionsLoading] = useState(false)
  const [liquidatingBorrower, setLiquidatingBorrower] = useState<string | null>(null)

  // Short position state
  const [shortPosition, setShortPosition] = useState<ShortPositionInfo | null>(null)
  const [shortLoading, setShortLoading] = useState(false)

  // All-shorts scan state (mirrors allPositions for the lending side).
  const [allShorts, setAllShorts] = useState<AllShortPositionsResult | null>(null)
  const [shortPositionsLoading, setShortPositionsLoading] = useState(false)
  const [liquidatingShorter, setLiquidatingShorter] = useState<string | null>(null)

  // Vault state
  const [userVault, setUserVault] = useState<VaultInfo | null>(null)
  const [useVault, setUseVault] = useState(false)
  // Wallet SOL balance (for short collateral presets when not using vault)
  const [walletSolBalance, setWalletSolBalance] = useState<number>(0)

  useEffect(() => {
    if (!wallet.publicKey) {
      setUserVault(null)
      setUseVault(false)
      setWalletSolBalance(0)
      return
    }
    let cancelled = false
    getVault(connection, wallet.publicKey.toString())
      .then((v) => { if (!cancelled) setUserVault(v) })
      .catch(() => { if (!cancelled) setUserVault(null) })
    connection
      .getBalance(wallet.publicKey)
      .then((lamports) => { if (!cancelled) setWalletSolBalance(lamports / LAMPORTS_PER_SOL) })
      .catch(() => { if (!cancelled) setWalletSolBalance(0) })
    return () => { cancelled = true }
  }, [connection, wallet.publicKey])

  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [marginMode, setMarginMode] = useState<'lend' | 'short'>('lend')
  const [actionTab, setActionTab] = useState<'borrow' | 'repay'>('borrow')
  const [shortTab, setShortTab] = useState<'open' | 'close'>('open')
  const [collateralAmount, setCollateralAmount] = useState('')
  const [borrowAmount, setBorrowAmount] = useState('')
  const [repayAmount, setRepayAmount] = useState('')
  // Short selling state
  const [shortCollateralSol, setShortCollateralSol] = useState('')
  const [shortTokenAmount, setShortTokenAmount] = useState('')
  const [closeTokenAmount, setCloseTokenAmount] = useState('')

  const [actionLoading, setActionLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // Computed values
  const userBalanceTokens = Number(userTokenBalance) / TOKEN_MULTIPLIER
  const collateralParsed = parseFloat(collateralAmount) || 0
  const collateralValueSol = collateralParsed * priceInSol
  const maxBorrowLtv = lendingInfo ? collateralValueSol * (lendingInfo.max_ltv_bps / 10000) : 0
  // Short side shares the same depth-aware ceiling (both flows call
  // `get_depth_max_ltv_bps(pool_sol)` on-chain against the DEX pool).
  // Fallback to the lowest depth tier (25%) when lendingInfo is unavailable
  // so the UI is never accidentally permissive vs. the on-chain check.
  const shortMaxLtvRatio = (lendingInfo?.max_ltv_bps ?? 2500) / 10000
  // Use on-chain treasury data (correct utilizationCapBps) instead of SDK's hardcoded cap
  const correctAvailableSol = Math.max(0, treasurySolBalance * utilizationCapBps / 10000 - totalSolLent)
  const treasuryAvailableSol = lendingInfo ? correctAvailableSol : 0
  // Per-user cap: max borrow = maxLendable * collateral * 3 / TOTAL_SUPPLY
  // treasurySolBalance is in SOL, collateral in display units, TOTAL_SUPPLY in display units
  const TOTAL_SUPPLY_TOKENS = 1_000_000_000 // 1B tokens (display units)
  const maxLendableSol = treasurySolBalance * utilizationCapBps / 10000
  // Account for Token-2022 transfer fee (4 bps) reducing net collateral on-chain
  const netCollateralTokens = collateralParsed * (1 - 4 / 10000)
  const borrowMultiplier = lendingInfo?.borrow_share_multiplier || 3
  const perUserCapSol = lendingInfo
    ? maxLendableSol * netCollateralTokens * borrowMultiplier / TOTAL_SUPPLY_TOKENS
    : 0
  const effectiveMaxBorrow = Math.min(maxBorrowLtv, treasuryAvailableSol, perUserCapSol > 0 ? perUserCapSol : Infinity)
  const borrowParsed = parseFloat(borrowAmount) || 0
  const projectedLtvBps =
    collateralValueSol > 0 ? (borrowParsed / collateralValueSol) * 10000 : 0

  const fetchLendingInfo = useCallback(async () => {
    if (!isMigrated) return
    setLendingLoading(true)
    try {
      const info = await getLendingInfo(connection, mintAddress)
      setLendingInfo(info)
    } catch (err) {
      if (isDev) console.error('Failed to fetch lending info:', err)
    } finally {
      setLendingLoading(false)
    }
  }, [connection, mintAddress, isMigrated])

  const fetchLoanPosition = useCallback(async () => {
    if (!isMigrated || !wallet.publicKey) {
      setLoanPosition(null)
      return
    }
    setLoanLoading(true)
    try {
      const position = await getLoanPosition(connection, mintAddress, wallet.publicKey.toString())
      setLoanPosition(position)
    } catch (err) {
      if (isDev) console.error('Failed to fetch loan position:', err)
    } finally {
      setLoanLoading(false)
    }
  }, [connection, mintAddress, isMigrated, wallet.publicKey])

  const fetchAllPositions = useCallback(async () => {
    if (!isMigrated) return
    setPositionsLoading(true)
    try {
      const result = await getAllLoanPositions(connection, mintAddress)
      setAllPositions(result)
    } catch (err) {
      if (isDev) console.error('Failed to fetch all positions:', err)
    } finally {
      setPositionsLoading(false)
    }
  }, [connection, mintAddress, isMigrated])

  const fetchAllShortPositions = useCallback(async () => {
    if (!isMigrated) return
    setShortPositionsLoading(true)
    try {
      const result = await getAllShortPositions(connection, mintAddress)
      setAllShorts(result)
    } catch (err) {
      if (isDev) console.error('Failed to fetch all short positions:', err)
    } finally {
      setShortPositionsLoading(false)
    }
  }, [connection, mintAddress, isMigrated])

  useEffect(() => {
    fetchLendingInfo()
  }, [fetchLendingInfo])

  useEffect(() => {
    fetchLoanPosition()
  }, [fetchLoanPosition])

  useEffect(() => {
    fetchAllPositions()
  }, [fetchAllPositions])

  useEffect(() => {
    fetchAllShortPositions()
  }, [fetchAllShortPositions])

  const fetchShortPosition = useCallback(async () => {
    if (!isMigrated || !wallet.publicKey) {
      setShortPosition(null)
      return
    }
    setShortLoading(true)
    try {
      const position = await getShortPosition(connection, mintAddress, wallet.publicKey.toString())
      setShortPosition(position)
    } catch (err) {
      if (isDev) console.error('Failed to fetch short position:', err)
    } finally {
      setShortLoading(false)
    }
  }, [connection, mintAddress, isMigrated, wallet.publicKey])

  useEffect(() => {
    fetchShortPosition()
  }, [fetchShortPosition])

  // Not migrated state
  if (!isMigrated) {
    return (
      <div className="max-w-2xl mx-auto">
        <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-8 text-center">
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="48"
            height="48"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="mx-auto mb-4 text-white/30"
          >
            <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
            <path d="M7 11V7a5 5 0 0 1 10 0v4" />
          </svg>
          <h3 className="text-white text-lg font-semibold mb-2">Margin Not Available</h3>
          <p className="text-white/50 text-sm">
            Lending and short selling activate after migration to DEX. This token is still in the bonding phase.
          </p>
        </div>
      </div>
    )
  }

  async function handleBorrow() {
    if (!wallet.publicKey) return

    const collateral = parseFloat(collateralAmount)
    const solToBorrow = parseFloat(borrowAmount)
    if (isNaN(collateral) || collateral <= 0) {
      setError('Enter a valid collateral amount')
      return
    }
    if (isNaN(solToBorrow) || solToBorrow <= 0) {
      setError('Enter a valid SOL amount to borrow')
      return
    }

    setActionLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildBorrowTransaction(connection, {
        mint: mintAddress,
        borrower: wallet.publicKey.toString(),
        collateral_amount: Math.floor(collateral * TOKEN_MULTIPLIER),
        sol_to_borrow: Math.floor(solToBorrow * LAMPORTS_PER_SOL),
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      // Pre-simulate to get real error logs (Phantom hides them behind "Unexpected error")
      if (isDev) {
        try {
          const sim = await connection.simulateTransaction(transaction, { commitment: 'confirmed' })
          if (sim.value.err) {
            console.error('Borrow simulation failed:', sim.value.err)
            console.error('Logs:', sim.value.logs)
          }
        } catch (simErr) {
          console.error('Borrow simulation error:', simErr)
        }
      }

      const txId = await sendTransaction(transaction)

      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(
        connection,
        txId,
        latestBlockhash.blockhash,
        latestBlockhash.lastValidBlockHeight,
      )

      setCollateralAmount('')
      setBorrowAmount('')
      setSuccess('Borrow successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchLoanPosition()
      }, 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Borrow error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleRepay() {
    if (!wallet.publicKey) return

    const solAmount = parseFloat(repayAmount)
    if (isNaN(solAmount) || solAmount <= 0) {
      setError('Enter a valid SOL amount to repay')
      return
    }

    setActionLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildRepayTransaction(connection, {
        mint: mintAddress,
        borrower: wallet.publicKey.toString(),
        sol_amount: Math.floor(solAmount * LAMPORTS_PER_SOL),
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      const txId = await sendTransaction(transaction)

      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(
        connection,
        txId,
        latestBlockhash.blockhash,
        latestBlockhash.lastValidBlockHeight,
      )

      setRepayAmount('')
      const totalOwed = loanPosition?.total_owed ?? 0
      const isFullRepay = solAmount >= totalOwed / LAMPORTS_PER_SOL - 0.0001
      setSuccess(isFullRepay ? 'Loan fully repaid!' : 'Repayment successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchLoanPosition()
      }, 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Repay error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleLiquidate(borrower: string) {
    if (!wallet.publicKey) return

    setLiquidatingBorrower(borrower)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildLiquidateTransaction(connection, {
        mint: mintAddress,
        liquidator: wallet.publicKey.toString(),
        borrower,
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      const txId = await sendTransaction(transaction)

      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(
        connection,
        txId,
        latestBlockhash.blockhash,
        latestBlockhash.lastValidBlockHeight,
      )

      setSuccess('Liquidation successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchLoanPosition()
        fetchAllPositions()
      }, 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Liquidate error:', err)
      setError(parseLendingError(err))
    } finally {
      setLiquidatingBorrower(null)
    }
  }

  async function handleLiquidateShort(shorter: string) {
    if (!wallet.publicKey) return

    setLiquidatingShorter(shorter)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildLiquidateShortTransaction(connection, {
        mint: mintAddress,
        liquidator: wallet.publicKey.toString(),
        // LiquidateShortParams reuses the `borrower` field name from the
        // long-loan params shape; semantically this is the shorter.
        borrower: shorter,
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      const txId = await sendTransaction(transaction)
      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(
        connection,
        txId,
        latestBlockhash.blockhash,
        latestBlockhash.lastValidBlockHeight,
      )

      setSuccess('Short liquidation successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchShortPosition()
        fetchAllShortPositions()
      }, 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Liquidate short error:', err)
      setError(parseLendingError(err))
    } finally {
      setLiquidatingShorter(null)
    }
  }

  // Short selling handlers
  async function handleOpenShort() {
    if (!wallet.publicKey) return

    const solCollateral = parseFloat(shortCollateralSol)
    const tokensToBorrow = parseFloat(shortTokenAmount)
    if (isNaN(solCollateral) || solCollateral <= 0) {
      setError('Enter SOL collateral amount')
      return
    }
    if (isNaN(tokensToBorrow) || tokensToBorrow <= 0) {
      setError('Enter tokens to borrow')
      return
    }

    setActionLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildOpenShortTransaction(connection, {
        mint: mintAddress,
        shorter: wallet.publicKey.toString(),
        sol_collateral: Math.floor(solCollateral * LAMPORTS_PER_SOL),
        tokens_to_borrow: Math.floor(tokensToBorrow * TOKEN_MULTIPLIER),
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      const txId = await sendTransaction(transaction)
      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latestBlockhash.blockhash, latestBlockhash.lastValidBlockHeight)

      setShortCollateralSol('')
      setShortTokenAmount('')
      setSuccess('Short opened!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => fetchShortPosition(), 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Open short error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleCloseShort() {
    if (!wallet.publicKey) return

    const tokenAmount = parseFloat(closeTokenAmount)
    if (isNaN(tokenAmount) || tokenAmount <= 0) {
      setError('Enter token amount to return')
      return
    }

    setActionLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction } = await buildCloseShortTransaction(connection, {
        mint: mintAddress,
        shorter: wallet.publicKey.toString(),
        token_amount: Math.floor(tokenAmount * TOKEN_MULTIPLIER),
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      const txId = await sendTransaction(transaction)
      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latestBlockhash.blockhash, latestBlockhash.lastValidBlockHeight)

      setCloseTokenAmount('')
      setSuccess('Short closed!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => fetchShortPosition(), 2000)
    } catch (err: unknown) {
      if (isDev) console.error('Close short error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  const hasActivePosition = loanPosition && loanPosition.health !== 'none'

  const healthColor =
    loanPosition?.health === 'healthy'
      ? 'text-success'
      : loanPosition?.health === 'at_risk'
        ? 'text-accent'
        : loanPosition?.health === 'liquidatable'
          ? 'text-danger'
          : 'text-white/50'

  const hasPositions = allPositions && allPositions.positions.length > 0

  return (
    <div className="w-full max-w-2xl mx-auto space-y-3">
      {/* Lend / Short mode toggle */}
      <div className="flex gap-1">
        <button
          onClick={() => { setMarginMode('lend'); setError(null); setSuccess(null) }}
          className={`px-4 py-1.5 rounded-lg text-sm font-medium transition-all cursor-pointer ${
            marginMode === 'lend'
              ? 'bg-[color-mix(in_srgb,var(--accent)_15%,transparent)] text-[var(--accent)] border border-[var(--accent)]'
              : 'bg-[var(--surface)] text-white/50 border border-transparent hover:bg-[var(--surface-hover)]'
          }`}
        >
          Lend
        </button>
        <button
          onClick={() => { setMarginMode('short'); setError(null); setSuccess(null) }}
          className={`px-4 py-1.5 rounded-lg text-sm font-medium transition-all cursor-pointer ${
            marginMode === 'short'
              ? 'bg-[color-mix(in_srgb,var(--accent)_15%,transparent)] text-[var(--accent)] border border-[var(--accent)]'
              : 'bg-[var(--surface)] text-white/50 border border-transparent hover:bg-[var(--surface-hover)]'
          }`}
        >
          Short
        </button>
      </div>

      {marginMode === 'lend' ? (
      <>
      {/* Left column: Pool + Position + Borrow/Repay */}
      <div className="space-y-3">
      {/* Card 1: Pool + Position */}
      <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
        <h3 className="text-white font-semibold text-sm mb-3 flex items-center gap-2">
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <line x1="12" y1="1" x2="12" y2="23" />
            <path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6" />
          </svg>
          Lending Pool
        </h3>

        {lendingLoading ? (
          <p className="text-white/50 text-sm">Loading pool info...</p>
        ) : lendingInfo ? (
          <div className="space-y-2 text-xs">
            {/* 2-column grid for pool params */}
            <div className="grid grid-cols-2 gap-x-4 gap-y-1">
              <div className="flex justify-between">
                <span className="text-white/40">Interest</span>
                <span className="text-white font-mono">
                  {(lendingInfo.interest_rate_bps / 100).toFixed(1)}%/epoch
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/40">Max LTV</span>
                <span className="text-white font-mono">
                  {(lendingInfo.max_ltv_bps / 100).toFixed(0)}%
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/40">Liq Threshold</span>
                <span className="text-white font-mono">
                  {(lendingInfo.liquidation_threshold_bps / 100).toFixed(0)}%
                </span>
              </div>
              <div className="flex justify-between">
                <span className="text-white/40">Available</span>
                <span className="text-white font-mono">
                  {correctAvailableSol.toFixed(2)} SOL
                </span>
              </div>
              {collateralParsed > 0 && (
                <>
                  <div className="flex justify-between">
                    <span className="text-white/40">Your Max Borrow</span>
                    <span className="text-accent font-mono">
                      {effectiveMaxBorrow.toFixed(4)} SOL
                    </span>
                  </div>
                  {perUserCapSol > 0 && perUserCapSol < maxBorrowLtv && perUserCapSol < treasuryAvailableSol && (
                    <div className="col-span-2 text-[10px] text-white/30">
                      Limited by per-user cap ({(netCollateralTokens / TOTAL_SUPPLY_TOKENS * 100).toFixed(2)}% of supply × {borrowMultiplier} = {perUserCapSol.toFixed(4)} SOL max)
                    </div>
                  )}
                </>
              )}
            </div>

            {lendingInfo.warnings && lendingInfo.warnings.length > 0 && (
              <div>
                {lendingInfo.warnings.map((w, i) => (
                  <p key={i} className="text-accent text-xs">
                    {w}
                  </p>
                ))}
              </div>
            )}

            {/* How It Works */}
            <button
              onClick={() => setShowHowItWorks(!showHowItWorks)}
              className="text-[10px] text-white/30 hover:text-white/50 transition-colors cursor-pointer"
            >
              {showHowItWorks ? 'Hide' : 'How it works'}
            </button>
            {showHowItWorks && (
              <div className="text-[10px] text-white/30 space-y-1.5 bg-white/5 rounded-lg p-2.5">
                <p>Lock tokens as collateral to borrow SOL from the token&apos;s treasury. Your max borrow is the lowest of:</p>
                <ul className="list-disc pl-3.5 space-y-0.5">
                  <li><span className="text-white/50">LTV limit</span> — up to {lendingInfo.max_ltv_bps / 100}% of collateral value</li>
                  <li><span className="text-white/50">Pool available</span> — {(utilizationCapBps / 100).toFixed(0)}% of treasury SOL is lendable</li>
                  <li><span className="text-white/50">Per-user cap</span> — max 3x your collateral&apos;s share of total supply (e.g. 1% of supply = 3% of lendable pool)</li>
                </ul>
                <p>Interest accrues at {(lendingInfo.interest_rate_bps / 100).toFixed(1)}% per epoch (~7 days). Positions above {lendingInfo.liquidation_threshold_bps / 100}% LTV can be liquidated by anyone for a {lendingInfo.liquidation_bonus_bps / 100}% bonus.</p>
              </div>
            )}

            {/* Active position section */}
            {wallet.publicKey && (
              <>
                <div className="border-t border-white/10 my-2" />
                <div className="flex items-center justify-between mb-1">
                  <span className="text-white/60 text-xs font-medium">Your Position</span>
                  {hasActivePosition && (
                    <span className={`text-xs font-mono font-semibold ${healthColor}`}>
                      {loanPosition.health === 'at_risk' ? 'At Risk' : loanPosition.health}
                    </span>
                  )}
                </div>

                {loanLoading ? (
                  <p className="text-white/50 text-xs">Loading...</p>
                ) : hasActivePosition ? (
                  <div className="space-y-2">
                    {/* LTV bar visualization */}
                    {loanPosition.current_ltv_bps !== null && (
                      <LtvBar
                        currentLtvBps={loanPosition.current_ltv_bps}
                        maxLtvBps={lendingInfo.max_ltv_bps}
                        liquidationThresholdBps={lendingInfo.liquidation_threshold_bps}
                      />
                    )}
                    {/* Position data in 2-column grid */}
                    <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                      <div className="flex justify-between">
                        <span className="text-white/40">Collateral</span>
                        <span className="text-white font-mono">
                          {formatTokenValue(loanPosition.collateral_amount)} {symbol}
                        </span>
                      </div>
                      <div className="flex justify-between">
                        <span className="text-white/40">Borrowed</span>
                        <span className="text-white font-mono">
                          {formatSolValue(loanPosition.borrowed_amount)} SOL
                        </span>
                      </div>
                      <div className="flex justify-between">
                        <span className="text-white/40">Owed</span>
                        <span className="text-white font-mono font-semibold">
                          {formatSolValue(loanPosition.total_owed)} SOL
                        </span>
                      </div>
                      {loanPosition.current_ltv_bps !== null && (
                        <div className="flex justify-between">
                          <span className="text-white/40">LTV</span>
                          <span className="text-white font-mono">
                            {(loanPosition.current_ltv_bps / 100).toFixed(1)}%
                          </span>
                        </div>
                      )}
                    </div>
                    {loanPosition.warnings && loanPosition.warnings.length > 0 && (
                      <div>
                        {loanPosition.warnings.map((w, i) => (
                          <p key={i} className="text-accent text-xs">
                            {w}
                          </p>
                        ))}
                      </div>
                    )}
                  </div>
                ) : (
                  <p className="text-white/40 text-xs text-center py-1">No active loan</p>
                )}
              </>
            )}
          </div>
        ) : (
          <p className="text-white/40 text-sm">Unable to load pool info</p>
        )}
      </div>

      {/* Card 2: Enhanced Borrow/Repay Form */}
      <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
        {/* Tabs */}
        <div className="flex mb-4 border-b border-white/10">
          <button
            onClick={() => {
              setActionTab('borrow')
              setError(null)
              setSuccess(null)
            }}
            className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
              actionTab === 'borrow'
                ? 'text-accent border-b-2 border-accent'
                : 'text-white/50 hover:text-white/70'
            }`}
          >
            Borrow
          </button>
          <button
            onClick={() => {
              setActionTab('repay')
              setError(null)
              setSuccess(null)
            }}
            className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
              actionTab === 'repay'
                ? 'text-accent border-b-2 border-accent'
                : 'text-white/50 hover:text-white/70'
            }`}
          >
            Repay
          </button>
        </div>

        {actionTab === 'borrow' ? (
          <div className="space-y-3">
            {/* Collateral input */}
            <div>
              <label className="block text-sm text-white/50 mb-1.5">
                Collateral ({symbol})
              </label>
              <input
                type="number"
                value={collateralAmount}
                onChange={(e) => setCollateralAmount(e.target.value)}
                placeholder={`0 ${symbol}`}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              {/* Collateral % presets — base on vault or wallet balance per the active route */}
              {(() => {
                const source =
                  useVault && userVault && vaultTokenBalance && vaultTokenBalance > BigInt(0)
                    ? vaultTokenBalance
                    : userTokenBalance
                if (source <= BigInt(0)) return null
                return (
                  <div className="flex gap-2 mt-2">
                    {[25, 50, 75, 100].map((pct) => (
                      <button
                        key={pct}
                        onClick={() => {
                          const tokenAmount = (source * BigInt(pct)) / BigInt(100)
                          setCollateralAmount(
                            (Number(tokenAmount) / TOKEN_MULTIPLIER).toString(),
                          )
                        }}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {pct === 100 ? 'Max' : `${pct}%`}
                      </button>
                    ))}
                  </div>
                )
              })()}
            </div>

            {/* SOL borrow input */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-sm text-white/50">SOL to Borrow</label>
                {collateralParsed > 0 && lendingInfo && (
                  <span className="text-xs text-white/40 font-mono">
                    Max: {effectiveMaxBorrow.toFixed(4)} SOL
                  </span>
                )}
              </div>
              <input
                type="number"
                value={borrowAmount}
                onChange={(e) => setBorrowAmount(e.target.value)}
                placeholder="0.0 SOL"
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              {/* Borrow presets */}
              {collateralParsed > 0 && lendingInfo && effectiveMaxBorrow > 0 && (
                <div className="flex gap-2 mt-2">
                  {[
                    { label: '25%', fraction: 0.25 },
                    { label: '50%', fraction: 0.50 },
                    { label: 'Max', fraction: 1.0 },
                  ].map(({ label, fraction }) => {
                    const solValue = effectiveMaxBorrow * fraction
                    return (
                      <button
                        key={label}
                        onClick={() => setBorrowAmount(solValue.toFixed(4))}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {label}
                      </button>
                    )
                  })}
                </div>
              )}
            </div>

            {/* Projected LTV bar */}
            {borrowParsed > 0 && collateralValueSol > 0 && lendingInfo && (
              <div className="space-y-1">
                <div className="flex items-center justify-between">
                  <span className="text-xs text-white/40">Projected LTV</span>
                  <span
                    className={`text-xs font-mono font-semibold ${
                      projectedLtvBps > lendingInfo.max_ltv_bps
                        ? 'text-danger'
                        : projectedLtvBps > lendingInfo.max_ltv_bps * 0.75
                          ? 'text-accent'
                          : 'text-success'
                    }`}
                  >
                    {(projectedLtvBps / 100).toFixed(1)}%
                  </span>
                </div>
                <LtvBar
                  currentLtvBps={projectedLtvBps}
                  maxLtvBps={lendingInfo.max_ltv_bps}
                  liquidationThresholdBps={lendingInfo.liquidation_threshold_bps}
                />
                {projectedLtvBps > lendingInfo.max_ltv_bps && (
                  <p className="text-danger text-xs">
                    Exceeds max LTV — reduce borrow amount or add more collateral
                  </p>
                )}
              </div>
            )}

            {/* Vault Toggle */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Borrow via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'Collateral from vault, SOL to vault' : 'Direct wallet borrow'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {error && <p className="text-danger text-sm">{error}</p>}
            {success && <p className="text-success text-sm">{success}</p>}

            {wallet.publicKey ? (
              <button
                onClick={handleBorrow}
                disabled={actionLoading || !collateralAmount || !borrowAmount}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/15 text-white hover:bg-white/25"
              >
                {actionLoading ? 'Processing...' : 'Borrow'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to borrow</p>
            )}
          </div>
        ) : (
          <div className="space-y-3">
            {/* SOL repay input */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-sm text-white/50">SOL to Repay</label>
                {hasActivePosition && (
                  <span className="text-xs text-white/40 font-mono">
                    Owed: {formatSolValue(loanPosition.total_owed)} SOL
                  </span>
                )}
              </div>
              <input
                type="number"
                value={repayAmount}
                onChange={(e) => setRepayAmount(e.target.value)}
                placeholder="0.0 SOL"
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              {/* Repay % presets */}
              {hasActivePosition && loanPosition.total_owed > 0 && (
                <div className="flex gap-2 mt-2">
                  {[25, 50, 75, 100].map((pct) => {
                    // Add 1% buffer on full repay to account for interest accrual between quote and execution
                    const multiplier = pct === 100 ? 1.01 : 1
                    const solValue = (loanPosition.total_owed * pct * multiplier) / (100 * LAMPORTS_PER_SOL)
                    return (
                      <button
                        key={pct}
                        onClick={() => setRepayAmount(solValue.toFixed(4))}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {pct === 100 ? 'Max' : `${pct}%`}
                      </button>
                    )
                  })}
                </div>
              )}
            </div>

            {/* Vault Toggle */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Repay via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'SOL from vault, collateral to vault' : 'Direct wallet repay'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {error && <p className="text-danger text-sm">{error}</p>}
            {success && <p className="text-success text-sm">{success}</p>}

            {wallet.publicKey ? (
              <button
                onClick={handleRepay}
                disabled={actionLoading || !repayAmount}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/15 text-white hover:bg-white/25"
              >
                {actionLoading ? 'Processing...' : 'Repay'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to repay</p>
            )}
          </div>
        )}
      </div>
      </div>{/* end left column */}

      {/* Right column: Active Loans */}
      {hasPositions && (
        <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
          <h3 className="text-white font-semibold text-sm mb-3 flex items-center gap-2">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
              <circle cx="9" cy="7" r="4" />
              <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
              <path d="M16 3.13a4 4 0 0 1 0 7.75" />
            </svg>
            Active Loans ({allPositions!.positions.length})
          </h3>
          <p className="text-white/30 text-[10px] mb-3 -mt-1">
            Positions above 65% LTV can be liquidated. Liquidators repay the debt and receive the collateral at a 10% bonus.
          </p>

          {/* Table header */}
          <div className="grid grid-cols-[1fr_1fr_0.8fr_0.6fr_auto] gap-2 text-[10px] text-white/40 lowercase tracking-wider pb-1.5 border-b border-white/10">
            <span>Borrower</span>
            <span className="text-right">Collateral</span>
            <span className="text-right">Owed</span>
            <span className="text-right">LTV</span>
            <span className="text-right">Status</span>
          </div>

          {/* Table rows */}
          <div className="divide-y divide-white/5">
            {allPositions!.positions.map((pos) => {
              const ltvPct = pos.current_ltv_bps !== null ? pos.current_ltv_bps / 100 : null
              const ltvColor =
                ltvPct === null
                  ? 'text-white/50'
                  : ltvPct >= 65
                    ? 'text-danger'
                    : ltvPct >= 50
                      ? 'text-accent'
                      : 'text-success'
              const shortenedAddr = pos.borrower.slice(0, 4) + '...' + pos.borrower.slice(-2)
              const isOwnPosition = wallet.publicKey?.toString() === pos.borrower
              const canLiquidate =
                pos.health === 'liquidatable' &&
                wallet.publicKey &&
                !isOwnPosition

              return (
                <div
                  key={pos.borrower}
                  className="grid grid-cols-[1fr_1fr_0.8fr_0.6fr_auto] gap-2 items-center py-2 text-xs"
                >
                  <a
                    href={`https://solscan.io/account/${pos.borrower}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-accent hover:underline font-mono"
                  >
                    {shortenedAddr}
                  </a>
                  <span className="text-right text-white font-mono">
                    {formatTokenValue(pos.collateral_amount)} {symbol}
                  </span>
                  <span className="text-right text-white font-mono">
                    {formatSolValue(pos.total_owed)} SOL
                  </span>
                  <span className={`text-right font-mono ${ltvColor}`}>
                    {ltvPct !== null ? `${ltvPct.toFixed(0)}%` : '—'}
                  </span>
                  <span className="text-right">
                    {canLiquidate ? (
                      <button
                        onClick={() => handleLiquidate(pos.borrower)}
                        disabled={liquidatingBorrower !== null}
                        className="px-2 py-0.5 text-[10px] font-semibold rounded bg-red-500/20 text-danger hover:bg-red-500/30 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                      >
                        {liquidatingBorrower === pos.borrower ? 'Liquidating...' : 'Liquidate'}
                      </button>
                    ) : (
                      <span
                        className={`text-[10px] font-semibold ${
                          pos.health === 'liquidatable'
                            ? 'text-danger'
                            : pos.health === 'at_risk'
                              ? 'text-accent'
                              : 'text-success'
                        }`}
                      >
                        {pos.health === 'at_risk' ? 'At Risk' : pos.health === 'liquidatable' ? 'Underwater' : 'Healthy'}
                      </span>
                    )}
                  </span>
                </div>
              )
            })}
          </div>

          {error && liquidatingBorrower === null && (
            <p className="text-danger text-xs mt-2">{error}</p>
          )}
          {success && success.includes('Liquidation') && (
            <p className="text-success text-xs mt-2">{success}</p>
          )}
        </div>
      )}

      {positionsLoading && !hasPositions && (
        <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
          <p className="text-white/50 text-sm text-center">Loading active loans...</p>
        </div>
      )}
      </>
      ) : (
      /* ============================================================
         SHORT SELLING UI — mirrors lending layout exactly
         ============================================================ */
      <div className="space-y-3">
      {/* Card 1: Short Pool + Position (mirrors Lending Pool card) */}
      <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
        <h3 className="text-white font-semibold text-sm mb-3 flex items-center gap-2">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="22 17 13.5 8.5 8.5 13.5 2 7" />
            <polyline points="16 17 22 17 22 11" />
          </svg>
          Short Pool
        </h3>

        <div className="space-y-2 text-xs">
          {/* Same param order as lending: Interest, Max LTV, Liq Threshold, then pool-specific */}
          <div className="grid grid-cols-2 gap-x-4 gap-y-1">
            <div className="flex justify-between">
              <span className="text-white/40">Interest</span>
              <span className="text-white font-mono">
                {lendingInfo ? `${(lendingInfo.interest_rate_bps / 100).toFixed(1)}%/epoch` : '—'}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-white/40">Max LTV</span>
              <span className="text-white font-mono">
                {lendingInfo ? `${(lendingInfo.max_ltv_bps / 100).toFixed(0)}%` : '—'}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-white/40">Liq Threshold</span>
              <span className="text-white font-mono">
                {lendingInfo ? `${(lendingInfo.liquidation_threshold_bps / 100).toFixed(0)}%` : '—'}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-white/40">Market price</span>
              <span className="text-white font-mono">{priceInSol < 0.000001 ? priceInSol.toExponential(2) : priceInSol.toFixed(6)} SOL</span>
            </div>
            {shortCollateralSol && parseFloat(shortCollateralSol) > 0 && (
              <div className="flex justify-between">
                <span className="text-white/40">Max Borrow</span>
                <span className="text-accent font-mono">
                  ~{(parseFloat(shortCollateralSol) / priceInSol * 0.5).toFixed(0)} {symbol}
                </span>
              </div>
            )}
          </div>

          {/* Collapsible How It Works */}
          <button
            onClick={() => setShowHowItWorks(!showHowItWorks)}
            className="text-[10px] text-white/30 hover:text-white/50 transition-colors cursor-pointer"
          >
            {showHowItWorks ? 'Hide' : 'How it works'}
          </button>
          {showHowItWorks && (
            <div className="text-[10px] text-white/30 space-y-1.5 bg-white/5 rounded-lg p-2.5">
              <p>Post SOL as collateral, borrow real ${symbol} from the 300M treasury lock. Sell on the market. If price drops, buy back cheaper and keep the difference.</p>
              <p>Shorts borrow real supply — they contribute to price discovery.</p>
              <p>Interest accrues at 2% per epoch (~7 days). Positions above 65% LTV can be liquidated by anyone for a 10% bonus.</p>
            </div>
          )}

          {/* Your Position section (inside same card, like lending) */}
          {wallet.publicKey && (
            <>
              <div className="border-t border-white/10 my-2" />
              <div className="flex items-center justify-between mb-1">
                <span className="text-white/60 text-xs font-medium">Your Position</span>
                {shortPosition && shortPosition.health !== 'none' && (
                  <span className={`text-xs font-mono font-semibold ${
                    shortPosition.health === 'healthy' ? 'text-success'
                      : shortPosition.health === 'at_risk' ? 'text-accent'
                      : shortPosition.health === 'liquidatable' ? 'text-danger'
                      : 'text-white/50'
                  }`}>
                    {shortPosition.health === 'at_risk' ? 'At Risk' : shortPosition.health}
                  </span>
                )}
              </div>

              {shortLoading ? (
                <p className="text-white/50 text-xs">Loading...</p>
              ) : shortPosition && shortPosition.health !== 'none' ? (
                <div className="space-y-2">
                  {shortPosition.current_ltv_bps !== null && (
                    <LtvBar
                      currentLtvBps={shortPosition.current_ltv_bps}
                      maxLtvBps={5000}
                      liquidationThresholdBps={6500}
                    />
                  )}
                  <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                    <div className="flex justify-between">
                      <span className="text-white/40">Collateral</span>
                      <span className="text-white font-mono">
                        {formatSolValue(shortPosition.sol_collateral)} SOL
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-white/40">Borrowed</span>
                      <span className="text-white font-mono">
                        {formatTokenValue(shortPosition.tokens_borrowed)} {symbol}
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-white/40">Owed</span>
                      <span className="text-white font-mono font-semibold">
                        {formatTokenValue(shortPosition.total_owed_tokens)} {symbol}
                      </span>
                    </div>
                    {shortPosition.current_ltv_bps !== null && (
                      <div className="flex justify-between">
                        <span className="text-white/40">LTV</span>
                        <span className="text-white font-mono">
                          {(shortPosition.current_ltv_bps / 100).toFixed(1)}%
                        </span>
                      </div>
                    )}
                  </div>
                </div>
              ) : (
                <p className="text-white/40 text-xs text-center py-1">No active short</p>
              )}
            </>
          )}
        </div>
      </div>

      {/* Card 2: Open/Close Short Form (mirrors Borrow/Repay card) */}
      <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
        <div className="flex mb-4 border-b border-white/10">
          <button
            onClick={() => { setShortTab('open'); setError(null); setSuccess(null) }}
            className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
              shortTab === 'open'
                ? 'text-accent border-b-2 border-accent'
                : 'text-white/50 hover:text-white/70'
            }`}
          >
            Open Short
          </button>
          <button
            onClick={() => { setShortTab('close'); setError(null); setSuccess(null) }}
            className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
              shortTab === 'close'
                ? 'text-accent border-b-2 border-accent'
                : 'text-white/50 hover:text-white/70'
            }`}
          >
            Close Short
          </button>
        </div>

        {shortTab === 'open' ? (
          <div className="space-y-3">
            {/* SOL Collateral input */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-sm text-white/50">SOL Collateral</label>
                {(() => {
                  const source = useVault && userVault ? userVault.sol_balance : walletSolBalance
                  return source > 0 ? (
                    <span className="text-xs text-white/40 font-mono">
                      Available: {source.toFixed(4)} SOL
                    </span>
                  ) : null
                })()}
              </div>
              <input
                type="number"
                value={shortCollateralSol}
                onChange={(e) => setShortCollateralSol(e.target.value)}
                placeholder="0.0 SOL"
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                step="0.1"
                min="0"
              />
              {/* SOL collateral presets — absolute amounts. Disabled when the
                  active source (vault SOL when toggled, wallet SOL otherwise,
                  minus a small fee buffer for wallet) can't cover the value. */}
              {(() => {
                const source = useVault && userVault ? userVault.sol_balance : walletSolBalance
                const usable = useVault ? source : Math.max(0, source - 0.01)
                if (usable <= 0) return null
                return (
                  <div className="flex gap-2 mt-2">
                    {[1, 2, 5, 10].map((sol) => {
                      const affordable = sol <= usable
                      return (
                        <button
                          key={sol}
                          onClick={() => setShortCollateralSol(sol.toString())}
                          disabled={!affordable}
                          className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20 disabled:opacity-30 disabled:cursor-not-allowed"
                        >
                          {sol} SOL
                        </button>
                      )
                    })}
                  </div>
                )
              })()}
            </div>

            {/* Amount to short input */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-sm text-white/50">Amount to short (${symbol})</label>
                {shortCollateralSol && parseFloat(shortCollateralSol) > 0 && (
                  <span className="text-xs text-white/40 font-mono">
                    Max: ~{(parseFloat(shortCollateralSol) * shortMaxLtvRatio / priceInSol).toFixed(0)} {symbol}
                  </span>
                )}
              </div>
              <input
                type="number"
                value={shortTokenAmount}
                onChange={(e) => setShortTokenAmount(e.target.value)}
                placeholder={`0 ${symbol}`}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                step="100"
                min="0"
              />
              {/* Borrow % presets — use depth-aware ceiling from lendingInfo */}
              {shortCollateralSol && parseFloat(shortCollateralSol) > 0 && priceInSol > 0 && (
                <div className="flex gap-2 mt-2">
                  {[
                    { label: '25%', fraction: 0.25 },
                    { label: '50%', fraction: 0.50 },
                    { label: '75%', fraction: 0.75 },
                    { label: 'Max', fraction: 1.0 },
                  ].map(({ label, fraction }) => {
                    // Theoretical max tokens at the depth-tier LTV ceiling. Max
                    // preset shaves 1% to leave room for price drift between
                    // quote and on-chain execution; otherwise the program's
                    // `new_ltv <= effective_max_ltv` check trips on rounding.
                    const ceilingTokens = parseFloat(shortCollateralSol) * shortMaxLtvRatio / priceInSol
                    const safetyMargin = fraction === 1.0 ? 0.99 : 1.0
                    const tokenValue = Math.floor(ceilingTokens * fraction * safetyMargin)
                    return (
                      <button
                        key={label}
                        onClick={() => setShortTokenAmount(tokenValue.toString())}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {label}
                      </button>
                    )
                  })}
                </div>
              )}
            </div>

            {/* Vault Toggle */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Short via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'SOL from vault, tokens to vault' : 'Direct wallet'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {error && <p className="text-danger text-sm">{error}</p>}
            {success && <p className="text-success text-sm">{success}</p>}

            {wallet.publicKey ? (
              <button
                onClick={handleOpenShort}
                disabled={actionLoading || !shortCollateralSol || !shortTokenAmount}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/15 text-white hover:bg-white/25"
              >
                {actionLoading ? 'Processing...' : 'Open Short'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to short</p>
            )}
          </div>
        ) : (
          <div className="space-y-3">
            {/* Amount to repay input */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-sm text-white/50">Amount to repay (${symbol})</label>
                {shortPosition && shortPosition.health !== 'none' && (
                  <span className="text-xs font-mono" style={{ color: 'var(--muted)' }}>
                    Owed: {formatTokenValue(shortPosition.total_owed_tokens)} {symbol}
                    {' · '}
                    {(() => {
                      const owed = shortPosition.total_owed_tokens
                      const held = Number(useVault && userVault && vaultTokenBalance && vaultTokenBalance > BigInt(0)
                        ? vaultTokenBalance
                        : userTokenBalance)
                      const short = held < owed
                      return (
                        <span style={{ color: short ? 'var(--danger)' : 'var(--muted)' }}>
                          You hold: {formatTokenValue(held)} {symbol}
                        </span>
                      )
                    })()}
                  </span>
                )}
              </div>
              <input
                type="number"
                value={closeTokenAmount}
                onChange={(e) => setCloseTokenAmount(e.target.value)}
                placeholder={`0 ${symbol}`}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                step="100"
                min="0"
              />
              {/* Close % presets */}
              {shortPosition && shortPosition.health !== 'none' && shortPosition.total_owed_tokens > 0 && (
                <div className="flex gap-2 mt-2">
                  {[25, 50, 75, 100].map((pct) => {
                    // Add 1% buffer on full close to cover interest accrual
                    const multiplier = pct === 100 ? 1.01 : 1
                    const tokenValue = (shortPosition.total_owed_tokens * pct * multiplier) / (100 * TOKEN_MULTIPLIER)
                    return (
                      <button
                        key={pct}
                        onClick={() => setCloseTokenAmount(tokenValue.toFixed(2))}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {pct === 100 ? 'Max' : `${pct}%`}
                      </button>
                    )
                  })}
                </div>
              )}
            </div>

            {/* Vault Toggle */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Close via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'Tokens from vault, SOL to vault' : 'Direct wallet'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {error && <p className="text-danger text-sm">{error}</p>}
            {success && <p className="text-success text-sm">{success}</p>}

            {wallet.publicKey ? (
              <button
                onClick={handleCloseShort}
                disabled={actionLoading || !closeTokenAmount}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/15 text-white hover:bg-white/25"
              >
                {actionLoading ? 'Processing...' : 'Close Short'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to close</p>
            )}
          </div>
        )}
      </div>

      {/* Card 3: Active Shorts (mirrors Active Loans card on the lending side) */}
      {allShorts && allShorts.positions.length > 0 && (
        <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
          <h3 className="text-white font-semibold text-sm mb-3 flex items-center gap-2">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="23 18 13.5 8.5 8.5 13.5 1 6" />
              <polyline points="17 18 23 18 23 12" />
            </svg>
            Active Shorts ({allShorts.positions.length})
          </h3>
          <p className="text-white/30 text-[10px] mb-3 -mt-1">
            Positions above 65% LTV can be liquidated. Liquidators repay token debt and seize SOL collateral at a 10% bonus.
          </p>

          <div className="grid grid-cols-[1fr_1fr_0.8fr_0.6fr_auto] gap-2 text-[10px] text-white/40 lowercase tracking-wider pb-1.5 border-b border-white/10">
            <span>Shorter</span>
            <span className="text-right">Collateral</span>
            <span className="text-right">Owed</span>
            <span className="text-right">LTV</span>
            <span className="text-right">Status</span>
          </div>

          <div className="divide-y divide-white/5">
            {allShorts.positions.map((pos) => {
              const ltvPct = pos.current_ltv_bps !== null ? pos.current_ltv_bps / 100 : null
              const ltvColor =
                ltvPct === null
                  ? 'text-white/50'
                  : ltvPct >= 65
                    ? 'text-danger'
                    : ltvPct >= 50
                      ? 'text-accent'
                      : 'text-success'
              const shortenedAddr = pos.shorter.slice(0, 4) + '...' + pos.shorter.slice(-2)
              const isOwnPosition = wallet.publicKey?.toString() === pos.shorter
              const canLiquidate =
                pos.health === 'liquidatable' && wallet.publicKey && !isOwnPosition

              return (
                <div
                  key={pos.shorter}
                  className="grid grid-cols-[1fr_1fr_0.8fr_0.6fr_auto] gap-2 items-center py-2 text-xs"
                >
                  <a
                    href={`https://solscan.io/account/${pos.shorter}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-accent hover:underline font-mono"
                  >
                    {shortenedAddr}
                  </a>
                  <span className="text-right text-white font-mono">
                    {formatSolValue(pos.sol_collateral)} SOL
                  </span>
                  <span className="text-right text-white font-mono">
                    {formatTokenValue(pos.total_owed_tokens)} {symbol}
                  </span>
                  <span className={`text-right font-mono ${ltvColor}`}>
                    {ltvPct !== null ? `${ltvPct.toFixed(0)}%` : '—'}
                  </span>
                  <span className="text-right">
                    {canLiquidate ? (
                      <button
                        onClick={() => handleLiquidateShort(pos.shorter)}
                        disabled={liquidatingShorter !== null}
                        className="px-2 py-0.5 text-[10px] font-semibold rounded bg-red-500/20 text-danger hover:bg-red-500/30 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                      >
                        {liquidatingShorter === pos.shorter ? 'Liquidating...' : 'Liquidate'}
                      </button>
                    ) : (
                      <span
                        className={`text-[10px] font-semibold ${
                          pos.health === 'liquidatable'
                            ? 'text-danger'
                            : pos.health === 'at_risk'
                              ? 'text-accent'
                              : 'text-success'
                        }`}
                      >
                        {pos.health === 'at_risk'
                          ? 'At Risk'
                          : pos.health === 'liquidatable'
                            ? 'Underwater'
                            : 'Healthy'}
                      </span>
                    )}
                  </span>
                </div>
              )
            })}
          </div>

          {error && liquidatingShorter === null && (
            <p className="text-danger text-xs mt-2">{error}</p>
          )}
          {success && success.includes('Short liquidation') && (
            <p className="text-success text-xs mt-2">{success}</p>
          )}
        </div>
      )}

      {shortPositionsLoading && (!allShorts || allShorts.positions.length === 0) && (
        <div className="bg-[var(--surface)] border border-[var(--border-color)] rounded-xl p-4">
          <p className="text-white/50 text-sm text-center">Loading active shorts...</p>
        </div>
      )}
      </div>
      )}
    </div>
  )
}
