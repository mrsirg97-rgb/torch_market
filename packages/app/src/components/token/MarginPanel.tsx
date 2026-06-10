'use client'

/**
 * MarginPanel — compressed swap-style margin UI.
 *
 * Sits in the Margin slot of the page's outer Trade/Margin toggle. Two
 * inner sub-tabs:
 *
 *   Short    — post SOL collateral, borrow tokens from the protocol's
 *              300M TreasuryLock. Available immediately at migration.
 *   Borrow   — post token collateral, borrow SOL from the treasury.
 *              Gated by MIN_TREASURY_SOL_FOR_LENDING (mainnet: 100 SOL of
 *              available SOL = treasury.sol_balance − short_collateral_reserved).
 *
 * Positions appear below the form. Each existing position (short + long)
 * gets one collapsed row showing the current state; click to expand the
 * close/repay UI inline. Rows with no position are not rendered (no
 * empty-state placeholders).
 *
 * SDK calls + tx confirmation pattern are copy-adapted from the prior
 * LendingDashboard implementation; user-confirmed "current logic is fine".
 */

import { useState, useEffect, useCallback } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import {
  getLendingInfo,
  getBorrowQuote,
  getPosition,
  buildOpenLongTransaction,
  buildCloseLongTransaction,
  buildOpenShortTransaction,
  buildCloseShortTransaction,
  getVault,
  grossUpForTransferFee,
} from 'torchsdk'
import type {
  LendingInfo,
  PositionInfo,
  VaultInfo,
} from 'torchsdk'
import { LAMPORTS_PER_SOL, TOKEN_MULTIPLIER } from '@/lib/constants'
import { useNetwork } from '@/lib/NetworkContext'

const isDev = process.env.NODE_ENV === 'development'

// ============================================================================
// Helpers
// ============================================================================

function parseLendingError(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  if (msg.includes('User rejected')) return 'Transaction cancelled.'
  const anchorMatch = msg.match(/Error Message:\s*(.+?)(?:\.|$)/)
  if (anchorMatch) return anchorMatch[1].trim()
  const anchorCodeMatch = msg.match(/AnchorError[^:]*:\s*(.+?)(?:\.|$)/)
  if (anchorCodeMatch) return anchorCodeMatch[1].trim()
  const simMatch = msg.match(/Program \w+ failed:\s*(.+?)(?:\.|$)/)
  if (simMatch) return `Simulation failed: ${simMatch[1].trim()}`
  const customMatch = msg.match(/custom program error:\s*(0x[0-9a-fA-F]+)/)
  if (customMatch) return `Program error: ${customMatch[1]}`
  const instrMatch = msg.match(/Error processing Instruction \d+:\s*(.+?)(?:\.|$)/)
  if (instrMatch) return instrMatch[1].trim()
  if (msg.includes('insufficient') || msg.includes('Insufficient')) return 'Insufficient funds'
  if (msg.length > 120) return msg.slice(0, 120) + '…'
  return msg
}

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
    // Block-height-exceeded is treated as benign — tx may still confirm later.
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

function LtvBar({
  currentLtvBps,
  maxLtvBps,
  liquidationThresholdBps,
}: {
  currentLtvBps: number
  maxLtvBps: number
  liquidationThresholdBps: number
}) {
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
      <div className="relative h-1.5 bg-white/10 rounded-full overflow-hidden">
        <div
          className={`absolute inset-y-0 left-0 rounded-full transition-all ${fillColor}`}
          style={{ width: `${fillPct}%` }}
        />
        <div
          className="absolute top-0 bottom-0 w-0.5 bg-yellow-400"
          style={{ left: `${maxLtvPct}%` }}
          title={`Max LTV: ${(maxLtvBps / 100).toFixed(0)}%`}
        />
        <div
          className="absolute top-0 bottom-0 w-0.5 bg-red-500"
          style={{ left: `${liqPct}%` }}
          title={`Liquidation: ${(liquidationThresholdBps / 100).toFixed(0)}%`}
        />
      </div>
      <div className="flex justify-between text-[10px] text-white/30">
        <span>{(currentLtvBps / 100).toFixed(1)}%</span>
        <span className="text-accent/70">{(maxLtvBps / 100).toFixed(0)}% Max</span>
        <span className="text-danger/70">{(liquidationThresholdBps / 100).toFixed(0)}% Liq</span>
      </div>
    </div>
  )
}

// ============================================================================
// Component
// ============================================================================

interface MarginPanelProps {
  mintAddress: string
  isMigrated: boolean
  symbol: string
  userTokenBalance: bigint
  vaultTokenBalance?: bigint | null
  priceInSol: number
  treasurySolBalance: number
  totalSolLent: number
}

export function MarginPanel({
  mintAddress,
  isMigrated,
  symbol,
  userTokenBalance,
  vaultTokenBalance,
  priceInSol,
  treasurySolBalance,
  totalSolLent,
}: MarginPanelProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const { effectiveIndexerUrl, lendingGateLamports } = useNetwork()
  const sendTransaction = useMwaSendTransaction()

  // ─── Data ────────────────────────────────────────────────────────────
  const [lendingInfo, setLendingInfo] = useState<LendingInfo | null>(null)
  // [V21] Depth-scaled rails for this pool — the EFFECTIVE max LTV (the depth
  // curve, not the flat treasury ceiling) and the size cap (ρ_max · pool SOL).
  // Pool-derived (independent of the user's input), so fetched once via a
  // zero-collateral borrow quote; the SDK is the single source of truth.
  const [rails, setRails] = useState<{ maxLtvBps: number; sizeCapSol: number } | null>(null)
  const [loanPosition, setLoanPosition] = useState<PositionInfo | null>(null)
  const [shortPosition, setShortPosition] = useState<PositionInfo | null>(null)
  const [userVault, setUserVault] = useState<VaultInfo | null>(null)
  const [useVault, setUseVault] = useState(false)
  const [walletSolBalance, setWalletSolBalance] = useState<number>(0)

  // ─── UI state ────────────────────────────────────────────────────────
  const [mode, setMode] = useState<'short' | 'borrow'>('short')
  const [collateralAmount, setCollateralAmount] = useState('') // borrow: tokens
  const [borrowAmount, setBorrowAmount] = useState('') // borrow: SOL
  const [repayAmount, setRepayAmount] = useState('')
  const [shortCollateralSol, setShortCollateralSol] = useState('')
  const [shortTokenAmount, setShortTokenAmount] = useState('')
  const [closeTokenAmount, setCloseTokenAmount] = useState('')
  // Each position can expand inline for close/repay
  const [expandLong, setExpandLong] = useState(false)
  const [expandShort, setExpandShort] = useState(false)

  const [actionLoading, setActionLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // ─── Computed ────────────────────────────────────────────────────────
  const userBalanceTokens = Number(userTokenBalance) / TOKEN_MULTIPLIER
  const vaultBalanceTokens = vaultTokenBalance
    ? Number(vaultTokenBalance) / TOKEN_MULTIPLIER
    : 0

  // Borrow side
  const collateralParsed = parseFloat(collateralAmount) || 0
  const collateralValueSol = collateralParsed * priceInSol
  // [V21] Effective max LTV = the depth curve (rails), NOT the flat treasury
  // ceiling — on a thin pool this is ~30%, not 60%. Falls back to the ceiling,
  // then LTV_MIN, until the rails quote resolves.
  const maxLtvBps = rails?.maxLtvBps ?? lendingInfo?.max_ltv_bps ?? 3000
  // [V21] Rail 2 size cap: a position's SOL-debt-value ≤ ρ_max · pool depth.
  const sizeCapSol = rails?.sizeCapSol ?? Infinity
  const liquidationThresholdBps = lendingInfo?.liquidation_threshold_bps ?? 6500
  const maxBorrowLtv = collateralValueSol * (maxLtvBps / 10000)
  // [F-2] Capacity = the physical float, first-come-first-serve. The vault
  // lamports are ALREADY net of lent SOL (it physically leaves at open), so
  // there is no utilization multiplier and no lent subtraction.
  const correctAvailableSol = Math.max(0, treasurySolBalance)
  const treasuryAvailableSol = lendingInfo ? correctAvailableSol : 0
  // [V21] The per-user formula/absolute cap is enforced on-chain only — the SDK no
  // longer surfaces the multiplier/share constants. The estimate is bounded by the
  // depth-curve LTV, treasury headroom, and the Rail-2 size cap; the program
  // re-clamps by the per-user cap at open (rarely the binding constraint).
  const effectiveMaxBorrow = Math.min(
    maxBorrowLtv,
    treasuryAvailableSol,
    sizeCapSol, // [V21] Rail 2 size cap
  )
  const borrowParsed = parseFloat(borrowAmount) || 0
  const projectedBorrowLtvBps =
    collateralValueSol > 0 ? (borrowParsed / collateralValueSol) * 10000 : 0

  // Short side
  const shortCollateralParsed = parseFloat(shortCollateralSol) || 0
  const shortTokensParsed = parseFloat(shortTokenAmount) || 0
  const shortDebtValueSol = shortTokensParsed * priceInSol
  const shortMaxLtvRatio = maxLtvBps / 10000 // [V21] effective depth-curve LTV
  const projectedShortLtvBps =
    shortCollateralParsed > 0 ? (shortDebtValueSol / shortCollateralParsed) * 10000 : 0
  // Borrow tokens up to the LTV ratio, then clamp by the size cap (debt VALUE in
  // SOL ≤ ρ_max · pool → tokens ≤ sizeCapSol / price).
  const shortMaxBorrow =
    shortCollateralParsed > 0 && priceInSol > 0
      ? Math.min(
          (shortCollateralParsed * shortMaxLtvRatio) / priceInSol,
          sizeCapSol / priceInSol,
        )
      : 0

  // ─── Fetchers ───────────────────────────────────────────────────────
  const fetchLendingInfo = useCallback(async () => {
    if (!isMigrated) return
    try {
      const info = await getLendingInfo(connection, mintAddress)
      setLendingInfo(info)
      // Pool-derived rails (effective depth-curve LTV + size cap). A 0-collateral
      // quote returns both without depending on the user's input.
      const q = await getBorrowQuote(connection, mintAddress, 0)
      setRails({ maxLtvBps: q.max_ltv_bps, sizeCapSol: q.size_cap_sol / LAMPORTS_PER_SOL })
    } catch (err) {
      if (isDev) console.error('Error fetching lending info:', err)
    }
  }, [connection, mintAddress, lendingGateLamports, isMigrated])

  const fetchLoanPosition = useCallback(async () => {
    if (!isMigrated || !wallet.publicKey) {
      setLoanPosition(null)
      return
    }
    try {
      const principal = useVault && userVault ? userVault.creator : wallet.publicKey.toString()
      const position = await getPosition(connection, mintAddress, principal, 'long')
      setLoanPosition(position)
    } catch (err) {
      if (isDev) console.error('Error fetching loan position:', err)
      setLoanPosition(null)
    }
  }, [connection, mintAddress, wallet.publicKey, isMigrated, useVault, userVault])

  const fetchShortPosition = useCallback(async () => {
    if (!isMigrated || !wallet.publicKey) {
      setShortPosition(null)
      return
    }
    try {
      const principal = useVault && userVault ? userVault.creator : wallet.publicKey.toString()
      const position = await getPosition(connection, mintAddress, principal, 'short')
      setShortPosition(position)
    } catch (err) {
      if (isDev) console.error('Error fetching short position:', err)
      setShortPosition(null)
    }
  }, [connection, mintAddress, wallet.publicKey, isMigrated, useVault, userVault])

  useEffect(() => {
    if (!wallet.publicKey) {
      setUserVault(null)
      setUseVault(false)
      setWalletSolBalance(0)
      return
    }
    let cancelled = false
    getVault(connection, wallet.publicKey.toString())
      .then((v) => {
        if (!cancelled) setUserVault(v)
      })
      .catch(() => {
        if (!cancelled) setUserVault(null)
      })
    connection
      .getBalance(wallet.publicKey)
      .then((lamports) => {
        if (!cancelled) setWalletSolBalance(lamports / LAMPORTS_PER_SOL)
      })
      .catch(() => {
        if (!cancelled) setWalletSolBalance(0)
      })
    return () => {
      cancelled = true
    }
  }, [connection, wallet.publicKey])

  useEffect(() => {
    fetchLendingInfo()
  }, [fetchLendingInfo])
  useEffect(() => {
    fetchLoanPosition()
  }, [fetchLoanPosition])
  useEffect(() => {
    fetchShortPosition()
  }, [fetchShortPosition])

  // Suppress unused-var warnings — kept for future use (vault token balance
  // surfacing, indexer-accelerated reads). Trivial cost to leave in scope.
  void effectiveIndexerUrl
  void vaultBalanceTokens

  // ─── Handlers ───────────────────────────────────────────────────────
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
      const { transaction } = await buildOpenLongTransaction(connection, {
        mint: mintAddress,
        borrower: wallet.publicKey.toString(),
        // [V21] open_long auto-clamps the borrow to collateral × depth-LTV (no
        // sol_to_borrow knob); just post token collateral.
        collateral: Math.floor(collateral * TOKEN_MULTIPLIER),
        vault: useVault && userVault ? userVault.creator : undefined,
      })
      const txId = await sendTransaction(transaction)
      const latest = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latest.blockhash, latest.lastValidBlockHeight)
      setCollateralAmount('')
      setBorrowAmount('')
      setSuccess('Borrow successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchLoanPosition()
      }, 2000)
    } catch (err) {
      if (isDev) console.error('Borrow error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleRepay() {
    if (!wallet.publicKey) return
    const sol = parseFloat(repayAmount)
    if (isNaN(sol) || sol <= 0) {
      setError('Enter a valid SOL amount to repay')
      return
    }
    setActionLoading(true)
    setError(null)
    setSuccess(null)
    try {
      // [V21] close_long takes a repay FRACTION (bps) of the debt, not a SOL amount.
      const owedSol = (loanPosition?.total_owed ?? 0) / LAMPORTS_PER_SOL
      const fractionBps = owedSol > 0
        ? Math.min(10000, Math.max(1, Math.round((sol / owedSol) * 10000)))
        : 10000
      const { transaction } = await buildCloseLongTransaction(connection, {
        mint: mintAddress,
        borrower: wallet.publicKey.toString(),
        repay_fraction_bps: fractionBps,
        vault: useVault && userVault ? userVault.creator : undefined,
      })
      const txId = await sendTransaction(transaction)
      const latest = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latest.blockhash, latest.lastValidBlockHeight)
      setRepayAmount('')
      const totalOwed = loanPosition?.total_owed ?? 0
      const isFull = sol >= totalOwed / LAMPORTS_PER_SOL - 0.0001
      setSuccess(isFull ? 'Loan fully repaid!' : 'Repayment successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => {
        fetchLendingInfo()
        fetchLoanPosition()
      }, 2000)
    } catch (err) {
      if (isDev) console.error('Repay error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleOpenShort() {
    if (!wallet.publicKey) return
    const sol = parseFloat(shortCollateralSol)
    const tokens = parseFloat(shortTokenAmount)
    if (isNaN(sol) || sol <= 0) {
      setError('Enter SOL collateral')
      return
    }
    if (isNaN(tokens) || tokens <= 0) {
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
        // [V21] open_short auto-clamps the borrow to collateral × depth-LTV (no
        // tokens_to_borrow knob); just post SOL collateral.
        collateral: Math.floor(sol * LAMPORTS_PER_SOL),
        vault: useVault && userVault ? userVault.creator : undefined,
      })
      const txId = await sendTransaction(transaction)
      const latest = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latest.blockhash, latest.lastValidBlockHeight)
      setShortCollateralSol('')
      setShortTokenAmount('')
      setSuccess('Short opened!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => fetchShortPosition(), 2000)
    } catch (err) {
      if (isDev) console.error('Open short error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  async function handleCloseShort() {
    if (!wallet.publicKey) return
    const tokens = parseFloat(closeTokenAmount)
    if (isNaN(tokens) || tokens <= 0) {
      setError('Enter token amount to return')
      return
    }
    setActionLoading(true)
    setError(null)
    setSuccess(null)
    try {
      // [V21] close_short takes a repay FRACTION (bps), not a token amount.
      // Convert the entered token amount to a fraction of the total owed.
      const owedDisplay = (shortPosition?.total_owed ?? 0) / TOKEN_MULTIPLIER
      const fractionBps = owedDisplay > 0
        ? Math.min(10000, Math.max(1, Math.round((tokens / owedDisplay) * 10000)))
        : 10000
      const { transaction } = await buildCloseShortTransaction(connection, {
        mint: mintAddress,
        shorter: wallet.publicKey.toString(),
        repay_fraction_bps: fractionBps,
        vault: useVault && userVault ? userVault.creator : undefined,
      })
      const txId = await sendTransaction(transaction)
      const latest = await connection.getLatestBlockhash()
      await confirmTransactionSafe(connection, txId, latest.blockhash, latest.lastValidBlockHeight)
      setCloseTokenAmount('')
      const totalOwed = shortPosition?.total_owed ?? 0
      const isFull = tokens >= totalOwed / TOKEN_MULTIPLIER - 0.0001
      setSuccess(isFull ? 'Short fully closed!' : 'Partial close successful!')
      setTimeout(() => setSuccess(null), 3000)
      setTimeout(() => fetchShortPosition(), 2000)
    } catch (err) {
      if (isDev) console.error('Close short error:', err)
      setError(parseLendingError(err))
    } finally {
      setActionLoading(false)
    }
  }

  // ─── Early states ───────────────────────────────────────────────────
  if (!isMigrated) {
    return (
      <div className="card p-4 text-center">
        <p className="text-white/50 text-sm">Margin opens at migration.</p>
        <p className="text-white/30 text-xs mt-1">
          Short selling unlocks the instant this token graduates to the DEX.
        </p>
      </div>
    )
  }

  if (!wallet.publicKey) {
    return (
      <div className="card p-4 text-center">
        <p className="text-white/50 text-sm">Connect wallet to trade margin.</p>
      </div>
    )
  }

  // [V21] LendingInfo no longer exposes the unlock fields — reconstruct the gate
  // from the lending flag, the live treasury vault balance, and the network's
  // gate threshold (MIN_TREASURY_SOL_FOR_LENDING via useNetwork).
  const gateAvailableSol = (lendingInfo?.treasury_sol_vault_lamports ?? 0) / LAMPORTS_PER_SOL
  const gateThresholdSol = lendingGateLamports / LAMPORTS_PER_SOL
  const lendingUnlocked = (lendingInfo?.lending_enabled ?? false) && gateAvailableSol >= gateThresholdSol

  // ─── Render ─────────────────────────────────────────────────────────
  return (
    <div className="card p-3 flex flex-col gap-3">
      {/* Inner mode toggle */}
      <div className="flex gap-1 rounded-lg bg-white/5 p-0.5">
        <button
          onClick={() => setMode('short')}
          className={`flex-1 py-1.5 text-sm font-medium rounded-md transition-colors cursor-pointer ${
            mode === 'short'
              ? 'bg-[color-mix(in_srgb,var(--accent)_22%,transparent)] text-[var(--accent)]'
              : 'text-white/50 hover:text-white/70'
          }`}
        >
          Short
        </button>
        <button
          onClick={() => setMode('borrow')}
          className={`flex-1 py-1.5 text-sm font-medium rounded-md transition-colors cursor-pointer ${
            mode === 'borrow'
              ? 'bg-[color-mix(in_srgb,var(--accent)_22%,transparent)] text-[var(--accent)]'
              : 'text-white/50 hover:text-white/70'
          }`}
        >
          Borrow
        </button>
      </div>

      {/* Lock gate banner — Borrow only, when locked */}
      {mode === 'borrow' && !lendingUnlocked && lendingInfo && (
        <div className="rounded-lg bg-white/[0.03] p-3 space-y-1.5">
          <div className="flex justify-between items-baseline">
            <span className="text-white/70 text-xs font-medium">Borrowing locked</span>
            <span className="text-white/50 text-[10px] font-mono">
              {gateAvailableSol.toFixed(2)} / {gateThresholdSol.toFixed(0)} SOL
            </span>
          </div>
          <div className="h-1.5 rounded-full bg-white/5 overflow-hidden">
            <div
              className="h-full bg-accent transition-all"
              style={{
                width: `${Math.min(100, gateAvailableSol / Math.max(gateThresholdSol, 0.0001) * 100)}%`,
              }}
            />
          </div>
          <p className="text-white/40 text-[10px]">
            Unlocks once treasury accumulates {gateThresholdSol.toFixed(0)} SOL of earned fees.
            Every short cycle + trade adds fees to the pool.
          </p>
        </div>
      )}

      {/* Vault toggle — same styled switch as SwapPanel */}
      {userVault && (
        <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
          <div>
            <p className="text-sm text-white/70">Trade via Vault</p>
            <p className="text-xs text-white/40">
              {useVault
                ? mode === 'short'
                  ? 'SOL from vault, tokens to vault'
                  : 'Tokens from vault, SOL to vault'
                : 'Direct wallet trade'}
            </p>
          </div>
          <button
            onClick={() => setUseVault(!useVault)}
            className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
              useVault ? 'bg-accent' : 'bg-white/20'
            }`}
            aria-label="Toggle vault routing"
          >
            <span
              className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                useVault ? 'translate-x-5' : ''
              }`}
            />
          </button>
        </div>
      )}

      {/* ─── SHORT FORM ─── */}
      {mode === 'short' && (
        <div className="space-y-2.5">
          <FormRow
            label="SOL Collateral"
            value={shortCollateralSol}
            onChange={setShortCollateralSol}
            suffix="SOL"
            balance={walletSolBalance}
            balanceLabel="Balance"
            onMax={() =>
              setShortCollateralSol(Math.max(0, walletSolBalance - 0.01).toFixed(4))
            }
            presets={[1, 2, 5, 10].map((sol) => ({
              label: `${sol} SOL`,
              onClick: () => setShortCollateralSol(sol.toString()),
            }))}
          />
          <FormRow
            label={`Tokens to borrow (${symbol})`}
            value={shortTokenAmount}
            onChange={setShortTokenAmount}
            suffix={symbol}
            balance={shortMaxBorrow}
            balanceLabel="Max @ depth-LTV"
            onMax={() => setShortTokenAmount(shortMaxBorrow.toFixed(2))}
            presets={
              shortMaxBorrow > 0
                ? [25, 50, 75, 100].map((pct) => ({
                    label: `${pct}%`,
                    onClick: () =>
                      setShortTokenAmount(((shortMaxBorrow * pct) / 100).toFixed(2)),
                  }))
                : undefined
            }
          />

          {shortCollateralParsed > 0 && shortTokensParsed > 0 && lendingInfo && (
            <LtvBar
              currentLtvBps={projectedShortLtvBps}
              maxLtvBps={maxLtvBps}
              liquidationThresholdBps={lendingInfo.liquidation_threshold_bps}
            />
          )}

          <button
            onClick={handleOpenShort}
            disabled={actionLoading || shortCollateralParsed <= 0 || shortTokensParsed <= 0}
            className="w-full py-2.5 rounded-lg font-semibold text-sm bg-accent text-black hover:opacity-90 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer transition-opacity"
          >
            {actionLoading ? 'Opening…' : 'Open Short'}
          </button>
        </div>
      )}

      {/* ─── BORROW FORM ─── */}
      {mode === 'borrow' && (
        <div className="space-y-2.5">
          <FormRow
            label={`Token Collateral (${symbol})`}
            value={collateralAmount}
            onChange={setCollateralAmount}
            suffix={symbol}
            balance={userBalanceTokens}
            balanceLabel="Wallet"
            onMax={() => setCollateralAmount(userBalanceTokens.toFixed(2))}
            disabled={!lendingUnlocked}
            presets={
              userBalanceTokens > 0
                ? [25, 50, 75, 100].map((pct) => ({
                    label: `${pct}%`,
                    onClick: () =>
                      setCollateralAmount(((userBalanceTokens * pct) / 100).toFixed(2)),
                  }))
                : undefined
            }
          />
          <FormRow
            label="SOL to borrow"
            value={borrowAmount}
            onChange={setBorrowAmount}
            suffix="SOL"
            balance={effectiveMaxBorrow}
            balanceLabel="Max"
            onMax={() => setBorrowAmount(effectiveMaxBorrow.toFixed(4))}
            disabled={!lendingUnlocked}
            presets={[1, 2, 5, 10].map((sol) => ({
              label: `${sol} SOL`,
              onClick: () => {
                // Clamp preset to user's actual cap so we never offer an
                // amount the program would reject anyway.
                const capped = Math.min(sol, effectiveMaxBorrow)
                setBorrowAmount(capped > 0 ? capped.toFixed(4) : '')
              },
            }))}
          />

          {collateralParsed > 0 && borrowParsed > 0 && lendingInfo && (
            <LtvBar
              currentLtvBps={projectedBorrowLtvBps}
              maxLtvBps={maxLtvBps}
              liquidationThresholdBps={liquidationThresholdBps}
            />
          )}
          {rails && Number.isFinite(sizeCapSol) && collateralParsed > 0 && (
            <div className="flex justify-between text-[10px] text-white/30 mt-1">
              <span>size cap (25% of pool depth)</span>
              <span
                className={
                  Math.abs(effectiveMaxBorrow - sizeCapSol) < 1e-6 ? 'text-accent/70' : ''
                }
              >
                {sizeCapSol.toFixed(2)} SOL max
              </span>
            </div>
          )}

          <button
            onClick={handleBorrow}
            disabled={
              actionLoading ||
              !lendingUnlocked ||
              collateralParsed <= 0 ||
              borrowParsed <= 0
            }
            className="w-full py-2.5 rounded-lg font-semibold text-sm bg-accent text-black hover:opacity-90 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer transition-opacity"
          >
            {actionLoading ? 'Borrowing…' : lendingUnlocked ? 'Borrow' : 'Locked'}
          </button>
        </div>
      )}

      {/* ─── Messages ─── */}
      {error && (
        <div className="rounded-md bg-danger/10 border border-danger/30 px-2.5 py-2 text-danger text-xs">
          {error}
        </div>
      )}
      {success && (
        <div className="rounded-md bg-success/10 border border-success/30 px-2.5 py-2 text-success text-xs">
          {success}
        </div>
      )}

      {/* ─── Positions ─── */}
      {(shortPosition?.health !== 'none' || loanPosition?.health !== 'none') && (
        <div className="space-y-1.5 pt-1 border-t border-white/5">
          <p className="text-white/40 text-[10px] uppercase tracking-wide mt-1">Your positions</p>

          {/* Short row */}
          {shortPosition && shortPosition.health !== 'none' && (
            <PositionRow
              kind="short"
              symbol={symbol}
              expanded={expandShort}
              onToggle={() => setExpandShort((v) => !v)}
              healthLabel={shortPosition.health}
              line1={`${(shortPosition.total_owed / TOKEN_MULTIPLIER).toLocaleString(undefined, { maximumFractionDigits: 2 })} ${symbol} owed`}
              line2={`${(shortPosition.collateral_amount / LAMPORTS_PER_SOL).toFixed(3)} SOL collat · LTV ${shortPosition.current_ltv_bps != null ? (shortPosition.current_ltv_bps / 100).toFixed(1) : '—'}%`}
            >
              <FormRow
                label={`Repay tokens (${symbol})`}
                value={closeTokenAmount}
                onChange={setCloseTokenAmount}
                suffix={symbol}
                onMax={() =>
                  setCloseTokenAmount(
                    (shortPosition.total_owed / TOKEN_MULTIPLIER).toFixed(2),
                  )
                }
                presets={[25, 50, 75, 100].map((pct) => ({
                  label: pct === 100 ? 'Max' : `${pct}%`,
                  onClick: () => {
                    const owedDisplay = shortPosition.total_owed / TOKEN_MULTIPLIER
                    // Add 1% buffer at 100% to cover interest accrual between
                    // typing the amount and the tx landing on-chain.
                    const multiplier = pct === 100 ? 1.01 : 1
                    setCloseTokenAmount(((owedDisplay * pct * multiplier) / 100).toFixed(2))
                  },
                }))}
              />
              {parseFloat(closeTokenAmount) > 0 && (
                <p className="text-white/40 text-[10px] font-mono">
                  Send{' '}
                  {(
                    grossUpForTransferFee(parseFloat(closeTokenAmount) * TOKEN_MULTIPLIER) /
                    TOKEN_MULTIPLIER
                  ).toFixed(2)}{' '}
                  {symbol} (clears {parseFloat(closeTokenAmount).toFixed(2)} debt + 0.07% fee)
                </p>
              )}
              <button
                onClick={handleCloseShort}
                disabled={actionLoading || parseFloat(closeTokenAmount) <= 0}
                className="w-full py-1.5 rounded-md text-xs font-medium bg-white/10 text-white hover:bg-white/20 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                {actionLoading ? 'Closing…' : 'Close Short'}
              </button>
            </PositionRow>
          )}

          {/* Long (borrow) row */}
          {loanPosition && loanPosition.health !== 'none' && (
            <PositionRow
              kind="long"
              symbol={symbol}
              expanded={expandLong}
              onToggle={() => setExpandLong((v) => !v)}
              healthLabel={loanPosition.health}
              line1={`${(loanPosition.total_owed / LAMPORTS_PER_SOL).toFixed(4)} SOL owed`}
              line2={`${(loanPosition.collateral_amount / TOKEN_MULTIPLIER).toLocaleString(undefined, { maximumFractionDigits: 2 })} ${symbol} collat · LTV ${loanPosition.current_ltv_bps != null ? (loanPosition.current_ltv_bps / 100).toFixed(1) : '—'}%`}
            >
              <FormRow
                label="Repay SOL"
                value={repayAmount}
                onChange={setRepayAmount}
                suffix="SOL"
                onMax={() => setRepayAmount((loanPosition.total_owed / LAMPORTS_PER_SOL).toFixed(4))}
                presets={[25, 50, 75, 100].map((pct) => ({
                  label: pct === 100 ? 'Max' : `${pct}%`,
                  onClick: () => {
                    const owedSol = loanPosition.total_owed / LAMPORTS_PER_SOL
                    // 1% buffer on full repay for interest accrued in flight.
                    const multiplier = pct === 100 ? 1.01 : 1
                    setRepayAmount(((owedSol * pct * multiplier) / 100).toFixed(4))
                  },
                }))}
              />
              <button
                onClick={handleRepay}
                disabled={actionLoading || parseFloat(repayAmount) <= 0}
                className="w-full py-1.5 rounded-md text-xs font-medium bg-white/10 text-white hover:bg-white/20 disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
              >
                {actionLoading ? 'Repaying…' : 'Repay'}
              </button>
            </PositionRow>
          )}
        </div>
      )}
    </div>
  )
}

// ============================================================================
// Internal helpers
// ============================================================================

function FormRow({
  label,
  value,
  onChange,
  suffix,
  balance,
  balanceLabel,
  onMax,
  disabled,
  presets,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  suffix: string
  balance?: number
  balanceLabel?: string
  onMax?: () => void
  disabled?: boolean
  /** Quick-fill chips shown under the input. Same visual style as
   *  SwapPanel's buy/sell preset rows. */
  presets?: { label: string; onClick: () => void }[]
}) {
  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <label className="text-xs text-white/50">{label}</label>
        {balance !== undefined && balanceLabel && (
          <button
            onClick={onMax}
            disabled={disabled}
            className="text-[10px] text-white/40 hover:text-white/70 font-mono cursor-pointer disabled:cursor-not-allowed"
          >
            {balanceLabel}: {balance.toLocaleString(undefined, { maximumFractionDigits: 4 })}
          </button>
        )}
      </div>
      <div className="relative">
        <input
          type="number"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
          placeholder="0"
          className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors disabled:opacity-50 disabled:cursor-not-allowed text-sm pr-12"
          step="any"
          min="0"
        />
        <span className="absolute right-3 top-1/2 -translate-y-1/2 text-xs text-white/40 pointer-events-none">
          {suffix}
        </span>
      </div>
      {presets && presets.length > 0 && (
        <div className="flex gap-2 mt-2">
          {presets.map((p) => (
            <button
              key={p.label}
              onClick={p.onClick}
              disabled={disabled}
              className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20 disabled:opacity-30 disabled:cursor-not-allowed"
            >
              {p.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

function PositionRow({
  kind,
  expanded,
  onToggle,
  healthLabel,
  line1,
  line2,
  children,
}: {
  kind: 'short' | 'long'
  symbol: string
  expanded: boolean
  onToggle: () => void
  healthLabel: string
  line1: string
  line2: string
  children: React.ReactNode
}) {
  const healthColor =
    healthLabel === 'liquidatable'
      ? 'text-danger'
      : healthLabel === 'at_risk'
        ? 'text-yellow-400'
        : 'text-white/60'

  return (
    <div className="rounded-lg bg-white/[0.03]">
      <button
        onClick={onToggle}
        className="w-full px-3 py-2 flex items-center justify-between cursor-pointer hover:bg-white/[0.02] rounded-lg"
      >
        <div className="text-left">
          <div className="flex items-center gap-2">
            <span className="text-[10px] font-semibold uppercase tracking-wide text-white/40">
              {kind === 'short' ? 'Short' : 'Long'}
            </span>
            <span className={`text-[10px] font-medium ${healthColor}`}>
              {healthLabel === 'at_risk' ? 'at risk' : healthLabel}
            </span>
          </div>
          <p className="text-xs text-white mt-0.5 font-mono">{line1}</p>
          <p className="text-[10px] text-white/40 mt-0.5">{line2}</p>
        </div>
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-white/40"
          style={{ transform: expanded ? 'rotate(180deg)' : 'rotate(0)', transition: 'transform 0.2s' }}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>
      {expanded && <div className="px-3 pb-3 pt-1 space-y-2 border-t border-white/5">{children}</div>}
    </div>
  )
}
