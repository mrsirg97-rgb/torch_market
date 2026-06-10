'use client'

import { useState, useEffect, useMemo, useCallback } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync } from '@solana/spl-token'
import {
  getBondingCurvePda,
  getTokenTreasuryPda,
  getTreasuryLockPda,
  getDeepPoolAccounts,
  getAllPositions,
  buildLiquidateLongTransaction,
  buildLiquidateShortTransaction,
  type PositionWithKey,
} from 'torchsdk'
import { PriceChart, VerifiedBadgeInline } from '@/components'
import type { PricePoint } from '@/lib/trades'
import {
  formatSol,
  formatTokens,
  shortenAddress,
  TOKEN_DECIMALS,
  TOKEN_2022_PROGRAM_ID,
} from '@/lib/constants'
import { useNetwork } from '@/lib/NetworkContext'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'

export interface ChartTabsMessage {
  signature: string
  sender: string
  memo: string
  timestamp?: number
}

interface ChartTabsProps {
  mint: PublicKey
  priceInSol: number
  solRaised: number
  solPriceUsd: number | null
  priceHistory: PricePoint[]
  messages: ChartTabsMessage[]
  saidVerifications: Map<string, { verified: boolean; trustTier: 'high' | 'medium' | 'low' | null }>
}

type TabId = 'chart' | 'messages' | 'trades' | 'holders' | 'bubbles' | 'liquidations'

interface TokenHolder {
  address: string
  balance: bigint
  percentage: number
}

const TAB_ICONS: Record<TabId, React.ReactNode> = {
  chart: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="12" y1="20" x2="12" y2="10" />
      <line x1="18" y1="20" x2="18" y2="4" />
      <line x1="6" y1="20" x2="6" y2="16" />
    </svg>
  ),
  messages: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    </svg>
  ),
  trades: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="17 1 21 5 17 9" />
      <path d="M3 11V9a4 4 0 0 1 4-4h14" />
      <polyline points="7 23 3 19 7 15" />
      <path d="M21 13v2a4 4 0 0 1-4 4H3" />
    </svg>
  ),
  holders: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
      <circle cx="9" cy="7" r="4" />
      <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
      <path d="M16 3.13a4 4 0 0 1 0 7.75" />
    </svg>
  ),
  bubbles: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="10" />
      <circle cx="12" cy="12" r="6" />
      <circle cx="12" cy="12" r="2" />
    </svg>
  ),
  liquidations: (
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
    </svg>
  ),
}

const TAB_LABELS: Record<TabId, string> = {
  chart: 'Chart',
  messages: 'Messages',
  trades: 'Trades',
  holders: 'Holders',
  bubbles: 'Bubbles',
  liquidations: 'Liquidatable',
}

const TAB_ORDER: TabId[] = ['chart', 'bubbles', 'holders', 'trades', 'messages', 'liquidations']

export function ChartTabs({
  mint,
  priceInSol,
  solRaised,
  solPriceUsd,
  priceHistory,
  messages,
  saidVerifications,
}: ChartTabsProps) {
  const { connection } = useConnection()
  const { effectiveIndexerUrl } = useNetwork()
  const [activeTab, setActiveTab] = useState<TabId>('chart')

  // Holders/bubbles state (shared — same underlying fetch)
  const [holders, setHolders] = useState<TokenHolder[]>([])
  const [holdersLoading, setHoldersLoading] = useState(false)
  const [holdersError, setHoldersError] = useState<string | null>(null)

  // Liquidatable positions state
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()
  const [liquidatableLoans, setLiquidatableLoans] = useState<PositionWithKey[]>([])
  const [liquidatableShorts, setLiquidatableShorts] = useState<PositionWithKey[]>([])
  const [liqLoading, setLiqLoading] = useState(false)
  const [liqError, setLiqError] = useState<string | null>(null)
  // Per-row liquidating state, keyed by borrower pubkey.
  const [liquidating, setLiquidating] = useState<Set<string>>(new Set())
  const [liqActionError, setLiqActionError] = useState<string | null>(null)

  // Get pool/vault addresses to filter out of holders list
  const excludedAddresses = useMemo(() => {
    const excluded = new Set<string>()

    const [bondingCurvePda] = getBondingCurvePda(mint)
    excluded.add(
      getAssociatedTokenAddressSync(mint, bondingCurvePda, true, TOKEN_2022_PROGRAM_ID).toString(),
    )

    const [treasuryPda] = getTokenTreasuryPda(mint)
    excluded.add(
      getAssociatedTokenAddressSync(mint, treasuryPda, true, TOKEN_2022_PROGRAM_ID).toString(),
    )

    try {
      const [treasuryLockPda] = getTreasuryLockPda(mint)
      excluded.add(
        getAssociatedTokenAddressSync(mint, treasuryLockPda, true, TOKEN_2022_PROGRAM_ID).toString(),
      )
    } catch {
      /* not derivable yet */
    }

    try {
      const { tokenVault } = getDeepPoolAccounts(mint)
      excluded.add(tokenVault.toString())
    } catch {
      /* pre-migration */
    }

    return excluded
  }, [mint])

  // Fetch holders when holders or bubbles tab is active
  useEffect(() => {
    if (activeTab !== 'holders' && activeTab !== 'bubbles') return
    if (holders.length > 0) return

    async function fetchHolders() {
      setHoldersLoading(true)
      setHoldersError(null)

      try {
        const response = await connection.getTokenLargestAccounts(mint, 'confirmed')
        const totalSupply = BigInt(1_000_000_000) * BigInt(10 ** TOKEN_DECIMALS)

        const filtered = response.value
          .filter((account) => account.uiAmount && account.uiAmount > 0)
          .filter((account) => !excludedAddresses.has(account.address.toString()))
          .slice(0, 20)

        const accountInfos = await connection.getMultipleParsedAccounts(
          filtered.map((a) => a.address),
        )

        const holderData: TokenHolder[] = filtered.map((account, i) => {
          const parsed = accountInfos.value[i]?.data
          const owner = parsed && 'parsed' in parsed ? parsed.parsed?.info?.owner : null
          return {
            address: owner || account.address.toString(),
            balance: BigInt(account.amount),
            percentage: (Number(account.amount) / Number(totalSupply)) * 100,
          }
        })

        setHolders(holderData)
      } catch (err) {
        console.error('Error fetching holders:', err)
        setHoldersError('Failed to load holders')
      } finally {
        setHoldersLoading(false)
      }
    }

    fetchHolders()
  }, [activeTab, connection, mint, holders.length, excludedAddresses])

  // Fetch liquidatable positions. Re-runs on tab open and after each
  // successful liquidation. Fresh each time — positions change as price
  // moves, so a stale cache misleads the hunter.
  const refetchLiquidatable = useCallback(async () => {
    setLiqLoading(true)
    setLiqError(null)
    try {
      const opts = effectiveIndexerUrl ? { indexer: effectiveIndexerUrl } : {}
      const [loans, shorts] = await Promise.all([
        getAllPositions(connection, mint.toString(), { ...opts, side: 'long' }),
        getAllPositions(connection, mint.toString(), { ...opts, side: 'short' }),
      ])
      setLiquidatableLoans(loans.positions.filter((p) => p.health === 'liquidatable'))
      setLiquidatableShorts(shorts.positions.filter((p) => p.health === 'liquidatable'))
    } catch (err) {
      console.error('Error fetching liquidatable positions:', err)
      setLiqError('Failed to load liquidatable positions')
    } finally {
      setLiqLoading(false)
    }
  }, [connection, mint, effectiveIndexerUrl])

  useEffect(() => {
    if (activeTab !== 'liquidations') return
    refetchLiquidatable()
  }, [activeTab, refetchLiquidatable])

  const handleLiquidate = useCallback(
    async (kind: 'long' | 'short', borrower: string) => {
      if (!wallet.publicKey) {
        setLiqActionError('Connect wallet to liquidate')
        return
      }
      setLiqActionError(null)
      setLiquidating((s) => new Set(s).add(borrower))
      try {
        const build = kind === 'long' ? buildLiquidateLongTransaction : buildLiquidateShortTransaction
        const { transaction } = await build(connection, {
          mint: mint.toString(),
          liquidator: wallet.publicKey.toString(),
          borrower,
        })
        await sendTransaction(transaction)
        await refetchLiquidatable()
      } catch (err) {
        console.error('Liquidation failed:', err)
        setLiqActionError(err instanceof Error ? err.message : 'Liquidation failed')
      } finally {
        setLiquidating((s) => {
          const next = new Set(s)
          next.delete(borrower)
          return next
        })
      }
    },
    [connection, mint, sendTransaction, wallet.publicKey, refetchLiquidatable],
  )

  return (
    <div className="h-full flex flex-col">
      {/* Desktop: pill row (full strip visible) */}
      <div className="hidden sm:flex gap-1 mb-3">
        {TAB_ORDER.map((id) => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer flex-shrink-0 ${
              activeTab === id
                ? 'bg-accent/20 text-accent'
                : 'bg-white/5 text-white/50 hover:bg-white/10 hover:text-white/70'
            }`}
          >
            {TAB_ICONS[id]}
            {TAB_LABELS[id]}
          </button>
        ))}
      </div>

      {/* Mobile: dropdown — same visual pattern as FilterDropdown +
          TimeframeDropdown elsewhere in the app */}
      <div className="sm:hidden mb-3">
        <ViewDropdown value={activeTab} onChange={setActiveTab} />
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0">
        {activeTab === 'chart' && (
          <PriceChart
            mint={mint.toBase58()}
            priceInSol={priceInSol}
            solRaised={solRaised}
            solPriceUsd={solPriceUsd}
            priceHistory={priceHistory}
          />
        )}
        {activeTab === 'messages' && (
          <MessagesView messages={messages} saidVerifications={saidVerifications} />
        )}
        {activeTab === 'trades' && <TradesView priceHistory={priceHistory} />}
        {activeTab === 'holders' && (
          <LeaderboardView holders={holders} loading={holdersLoading} error={holdersError} />
        )}
        {activeTab === 'bubbles' && (
          <BubbleMapView holders={holders} loading={holdersLoading} error={holdersError} />
        )}
        {activeTab === 'liquidations' && (
          <LiquidationsView
            loans={liquidatableLoans}
            shorts={liquidatableShorts}
            loading={liqLoading}
            error={liqError}
            actionError={liqActionError}
            liquidating={liquidating}
            onLiquidate={handleLiquidate}
            walletConnected={!!wallet.publicKey}
          />
        )}
      </div>
    </div>
  )
}

// ============================================================================
// Messages view
// ============================================================================

function MessagesView({
  messages,
  saidVerifications,
}: {
  messages: ChartTabsMessage[]
  saidVerifications: Map<string, { verified: boolean; trustTier: 'high' | 'medium' | 'low' | null }>
}) {
  if (messages.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-center">
        <div>
          <p className="text-white/40 text-sm">No messages yet</p>
          <p className="text-white/30 text-xs mt-1">
            Trade with a message to start the conversation
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto space-y-2 pr-1">
      {messages.map((msg) => (
        <div key={msg.signature} className="bg-white/5 rounded-lg p-3">
          <div className="flex items-start justify-between gap-2 mb-1">
            <span className="text-accent text-xs font-mono flex items-center gap-1">
              {shortenAddress(msg.sender)}
              {saidVerifications.get(msg.sender)?.verified && (
                <VerifiedBadgeInline
                  verified={true}
                  trustTier={saidVerifications.get(msg.sender)?.trustTier ?? null}
                />
              )}
            </span>
            <span className="text-white/30 text-xs">
              {msg.timestamp
                ? new Date(msg.timestamp * 1000).toLocaleString(undefined, {
                    month: 'short',
                    day: 'numeric',
                    hour: '2-digit',
                    minute: '2-digit',
                  })
                : ''}
            </span>
          </div>
          <p className="text-white text-sm">{msg.memo}</p>
          <a
            href={`https://solscan.io/tx/${msg.signature}`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-white/30 hover:text-white/50 text-xs mt-1 inline-block"
          >
            View tx
          </a>
        </div>
      ))}
    </div>
  )
}

// ============================================================================
// Trades view
// ============================================================================

function TradesView({ priceHistory }: { priceHistory: PricePoint[] }) {
  if (priceHistory.length === 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-white/40 text-sm">No trades yet</p>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto space-y-2 pr-1">
      {[...priceHistory].reverse().map((trade, i) => (
        <div key={i} className="bg-white/5 rounded-lg p-3">
          <div className="flex items-center justify-between gap-2 mb-1">
            <div className="flex items-center gap-2">
              <span
                className={`text-xs font-semibold ${
                  trade.isBuy ? 'text-success' : 'text-danger'
                }`}
              >
                {trade.isBuy ? 'BUY' : 'SELL'}
              </span>
              <a
                href={`https://solscan.io/account/${trade.trader}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-accent text-xs font-mono hover:underline"
              >
                {shortenAddress(trade.trader)}
              </a>
            </div>
            <span className="text-white/30 text-xs">
              {new Date(trade.timestamp * 1000).toLocaleString(undefined, {
                month: 'short',
                day: 'numeric',
                hour: '2-digit',
                minute: '2-digit',
              })}
            </span>
          </div>
          <div className="flex items-center gap-3 text-xs">
            <span className="text-white/50">
              Vol:{' '}
              <span className="text-white">
                {trade.volume.toLocaleString(undefined, {
                  minimumFractionDigits: 2,
                  maximumFractionDigits: 4,
                })}{' '}
                SOL
              </span>
            </span>
            <span className="text-white/50">
              Price: <span className="text-white">{trade.price.toFixed(10)} SOL</span>
            </span>
          </div>
        </div>
      ))}
    </div>
  )
}

// ============================================================================
// Liquidatable positions view
// ============================================================================

function LiquidationsView({
  loans,
  shorts,
  loading,
  error,
  actionError,
  liquidating,
  onLiquidate,
  walletConnected,
}: {
  loans: PositionWithKey[]
  shorts: PositionWithKey[]
  loading: boolean
  error: string | null
  actionError: string | null
  liquidating: Set<string>
  onLiquidate: (kind: 'long' | 'short', borrower: string) => void
  walletConnected: boolean
}) {
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-white/50 text-sm">Scanning positions…</p>
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-danger text-sm">{error}</p>
      </div>
    )
  }
  if (loans.length === 0 && shorts.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-center">
        <div>
          <p className="text-white/40 text-sm">No liquidatable positions</p>
          <p className="text-white/30 text-xs mt-1">
            All positions are above the 65% LTV threshold.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto space-y-2 pr-1">
      {actionError && (
        <div className="bg-danger/10 border border-danger/30 rounded-lg p-2 text-danger text-xs">
          {actionError}
        </div>
      )}
      {loans.map((p) => (
        <LiquidatableRow
          key={`loan-${p.owner}`}
          kind="long"
          borrower={p.owner}
          ltvBps={p.current_ltv_bps}
          collatLabel={formatTokens(BigInt(Math.floor(p.collateral_amount)))}
          debtLabel={`${formatSol(p.total_owed)} SOL`}
          liquidating={liquidating.has(p.owner)}
          walletConnected={walletConnected}
          onLiquidate={() => onLiquidate('long', p.owner)}
        />
      ))}
      {shorts.map((p) => (
        <LiquidatableRow
          key={`short-${p.owner}`}
          kind="short"
          borrower={p.owner}
          ltvBps={p.current_ltv_bps}
          collatLabel={`${formatSol(p.collateral_amount)} SOL`}
          debtLabel={formatTokens(BigInt(Math.floor(p.total_owed)))}
          liquidating={liquidating.has(p.owner)}
          walletConnected={walletConnected}
          onLiquidate={() => onLiquidate('short', p.owner)}
        />
      ))}
    </div>
  )
}

function LiquidatableRow({
  kind,
  borrower,
  ltvBps,
  collatLabel,
  debtLabel,
  liquidating,
  walletConnected,
  onLiquidate,
}: {
  kind: 'long' | 'short'
  borrower: string
  ltvBps: number | null
  collatLabel: string
  debtLabel: string
  liquidating: boolean
  walletConnected: boolean
  onLiquidate: () => void
}) {
  return (
    <div className="bg-white/5 rounded-lg p-3">
      <div className="flex items-center justify-between gap-2 mb-1">
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold text-danger">
            {kind === 'long' ? 'LONG · LIQUIDATABLE' : 'SHORT · LIQUIDATABLE'}
          </span>
          <a
            href={`https://solscan.io/account/${borrower}`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-accent text-xs font-mono hover:underline"
          >
            {shortenAddress(borrower)}
          </a>
        </div>
        <span className="text-white/30 text-xs font-mono">
          LTV {ltvBps != null ? (ltvBps / 100).toFixed(1) : '—'}%
        </span>
      </div>
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3 text-xs">
          <span className="text-white/50">
            Collat: <span className="text-white font-mono">{collatLabel}</span>
          </span>
          <span className="text-white/50">
            Debt: <span className="text-white font-mono">{debtLabel}</span>
          </span>
        </div>
        <button
          onClick={onLiquidate}
          disabled={!walletConnected || liquidating}
          className="px-3 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50"
          style={{
            background: 'color-mix(in srgb, var(--danger) 18%, transparent)',
            color: 'var(--danger)',
          }}
          title={!walletConnected ? 'Connect wallet to liquidate' : `Liquidate this ${kind}`}
        >
          {liquidating ? 'Liquidating…' : 'Liquidate'}
        </button>
      </div>
    </div>
  )
}

// ============================================================================
// Holders leaderboard
// ============================================================================

function LeaderboardView({
  holders,
  loading,
  error,
}: {
  holders: TokenHolder[]
  loading: boolean
  error: string | null
}) {
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">Loading holders...</div>
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-danger">{error}</div>
      </div>
    )
  }
  if (holders.length === 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">No holders found</div>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto">
      <table className="w-full text-sm">
        <thead className="sticky top-0 bg-[var(--background)]">
          <tr className="text-white/50 text-xs">
            <th className="text-left py-2 px-2">#</th>
            <th className="text-left py-2 px-2">Address</th>
            <th className="text-right py-2 px-2">Balance</th>
            <th className="text-right py-2 px-2">%</th>
          </tr>
        </thead>
        <tbody>
          {holders.map((holder, index) => (
            <tr
              key={holder.address}
              className="border-t border-white/5 hover:bg-white/5 transition-colors"
            >
              <td className="py-2 px-2 text-white/30">{index + 1}</td>
              <td className="py-2 px-2">
                <a
                  href={`https://solscan.io/account/${holder.address}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent hover:underline font-mono"
                >
                  {shortenAddress(holder.address)}
                </a>
              </td>
              <td className="py-2 px-2 text-right font-mono text-white/70">
                {formatTokens(holder.balance)}
              </td>
              <td className="py-2 px-2 text-right">
                <span
                  className={`font-mono ${
                    holder.percentage >= 2
                      ? 'text-danger'
                      : holder.percentage >= 1
                        ? 'text-yellow-400'
                        : 'text-white/50'
                  }`}
                >
                  {holder.percentage.toFixed(2)}%
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ============================================================================
// Bubble map
// ============================================================================

function BubbleMapView({
  holders,
  loading,
  error,
}: {
  holders: TokenHolder[]
  loading: boolean
  error: string | null
}) {
  const bubbles = useMemo(() => {
    if (holders.length === 0) return []

    const maxPercentage = Math.max(...holders.map((h) => h.percentage))
    const minSize = 30
    const maxSize = 120

    return holders.map((holder, index) => {
      const size = minSize + (holder.percentage / maxPercentage) * (maxSize - minSize)
      const angle = index * 0.8
      const radius = 100 + index * 15
      const x = 50 + Math.cos(angle) * (radius / 4)
      const y = 50 + Math.sin(angle) * (radius / 4)

      return {
        ...holder,
        size,
        x: Math.max(10, Math.min(90, x)),
        y: Math.max(10, Math.min(90, y)),
        color: getHolderColor(holder.percentage),
      }
    })
  }, [holders])

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">Loading distribution...</div>
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-danger">{error}</div>
      </div>
    )
  }
  if (holders.length === 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">No holders found</div>
      </div>
    )
  }

  return (
    <div className="h-full relative overflow-hidden">
      <div className="absolute top-2 right-2 bg-black/50 rounded-lg p-2 text-xs z-10">
        <div className="flex items-center gap-2 mb-1">
          <div className="w-3 h-3 rounded-full bg-danger" />
          <span className="text-white/70">≥2% (whale)</span>
        </div>
        <div className="flex items-center gap-2 mb-1">
          <div className="w-3 h-3 rounded-full bg-yellow-400" />
          <span className="text-white/70">≥1%</span>
        </div>
        <div className="flex items-center gap-2">
          <div className="w-3 h-3 rounded-full bg-accent" />
          <span className="text-white/70">&lt;1%</span>
        </div>
      </div>

      <svg viewBox="0 0 100 100" className="w-full h-full">
        {bubbles.map((bubble) => (
          <g key={bubble.address}>
            <circle
              cx={bubble.x}
              cy={bubble.y}
              r={bubble.size / 10}
              fill={bubble.color}
              fillOpacity={0.6}
              stroke={bubble.color}
              strokeWidth={0.3}
              className="transition-all duration-300 hover:fill-opacity-80 cursor-pointer"
            >
              <title>
                {shortenAddress(bubble.address)}
                {'\n'}
                {formatTokens(bubble.balance)} ({bubble.percentage.toFixed(2)}%)
              </title>
            </circle>
            {bubble.size > 60 && (
              <text
                x={bubble.x}
                y={bubble.y}
                textAnchor="middle"
                dominantBaseline="middle"
                fill="white"
                fontSize={1.5}
                fontFamily="monospace"
              >
                {bubble.percentage.toFixed(1)}%
              </text>
            )}
          </g>
        ))}
      </svg>

      <div className="absolute bottom-2 left-2 bg-black/50 rounded-lg p-2 text-xs">
        <p className="text-white/70">
          Top {holders.length} holders:{' '}
          <span className="text-white font-mono">
            {holders.reduce((sum, h) => sum + h.percentage, 0).toFixed(1)}%
          </span>
        </p>
      </div>
    </div>
  )
}

function getHolderColor(percentage: number): string {
  if (percentage >= 2) return '#ef4444'
  if (percentage >= 1) return '#facc15'
  return '#6ee7b7'
}

// ============================================================================
// Mobile dropdown — same visual language as FilterDropdown + TimeframeDropdown
// ============================================================================

function ViewDropdown({
  value,
  onChange,
}: {
  value: TabId
  onChange: (v: TabId) => void
}) {
  const [isOpen, setIsOpen] = useState(false)

  return (
    <div className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="btn text-xs px-3 py-2 inline-flex items-center gap-2 w-full justify-between"
      >
        <span className="flex items-center gap-1.5">
          {TAB_ICONS[value]}
          {TAB_LABELS[value]}
        </span>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={`transition-transform ${isOpen ? 'rotate-180' : ''}`}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>

      {isOpen && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} />
          <div
            className="absolute top-full left-0 right-0 mt-1 overflow-hidden z-20 rounded-xl"
            style={{
              background: 'color-mix(in srgb, var(--background) 60%, transparent)',
              backdropFilter: 'blur(8px)',
              WebkitBackdropFilter: 'blur(8px)',
              boxShadow: '0 8px 24px rgba(0,0,0,0.2)',
            }}
          >
            {TAB_ORDER.map((id) => (
              <button
                key={id}
                onClick={() => {
                  onChange(id)
                  setIsOpen(false)
                }}
                className={`w-full px-3 py-2 text-xs text-left flex items-center gap-1.5 hover:bg-white/10 transition-colors cursor-pointer ${
                  value === id ? 'bg-white/5 text-white' : 'text-white/70'
                }`}
              >
                {TAB_ICONS[id]}
                {TAB_LABELS[id]}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
