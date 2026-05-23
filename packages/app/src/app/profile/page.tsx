/**
 * /profile — Lifetime trading stats for the connected wallet.
 *
 * Robinhood-style portfolio history. Pulls realized-PnL data from the
 * indexer's `/api/user-pnl/:wallet` endpoint (FIFO cost basis across all
 * bonding + DEX trades) and joins per-mint metadata from `useTokens` for
 * display. Aggregate stats on top, per-mint breakdown below sorted by
 * absolute impact.
 *
 * Open enhancements (deferred):
 *  - Win rate (% mints / trades closing in profit). Need per-trade outcome
 *    pairs from the indexer; data exists but no current endpoint exposes it.
 *  - Cumulative P&L over time (line chart from trade history).
 *  - Activity heatmap. Lower priority — needs per-day aggregation endpoint.
 */
'use client'

import { useMemo } from 'react'
import dynamic from 'next/dynamic'
import Link from 'next/link'
import { useWallet } from '@solana/wallet-adapter-react'
import { Header } from '@/components'
import { useTokens } from '@/hooks/useTokens'
import { useUserPnl } from '@/hooks/useUserPnl'
import { shortenAddress } from '@/lib/constants'
import type { UserPnlByMint } from 'torchsdk'

const WalletMultiButton = dynamic(
  () => import('@solana/wallet-adapter-react-ui').then((mod) => mod.WalletMultiButton),
  { ssr: false },
)

const LAMPORTS_PER_SOL = 1_000_000_000
const TOKEN_MULTIPLIER = 1_000_000 // 6 decimals

function fmt(n: number, places = 2): string {
  if (Math.abs(n) >= 1000) return n.toFixed(0)
  if (Math.abs(n) >= 1) return n.toFixed(places)
  if (Math.abs(n) >= 0.001) return n.toFixed(4)
  if (Math.abs(n) > 0) return n.toExponential(2)
  return '0'
}

function fmtSigned(n: number): string {
  return (n >= 0 ? '+' : '') + fmt(n)
}

function fmtTokens(raw: number): string {
  const display = raw / TOKEN_MULTIPLIER
  if (display >= 1_000_000) return `${(display / 1_000_000).toFixed(2)}M`
  if (display >= 1_000) return `${(display / 1_000).toFixed(2)}K`
  return display.toFixed(2)
}

export default function ProfilePage() {
  const { publicKey } = useWallet()
  const { pnl, loading: pnlLoading } = useUserPnl()
  const { tokens } = useTokens({ enabled: !!publicKey })

  // Token-metadata lookup: mint → { name, symbol }
  const tokenMeta = useMemo(() => {
    const map = new Map<string, { name: string; symbol: string; image?: string }>()
    for (const t of tokens) {
      map.set(t.mint, { name: t.name, symbol: t.symbol, image: t.image })
    }
    return map
  }, [tokens])

  // Aggregate-level "wins vs losses" by mint — cheap heuristic before we
  // have proper per-trade P&L data. A "winning mint" has realized_pnl > 0.
  const aggregates = useMemo(() => {
    if (!pnl) return null
    let winningMints = 0
    let losingMints = 0
    let biggestWin: UserPnlByMint | null = null
    let biggestLoss: UserPnlByMint | null = null
    for (const entry of pnl.by_mint) {
      if (entry.realized_pnl > 0) {
        winningMints++
        if (!biggestWin || entry.realized_pnl > biggestWin.realized_pnl) {
          biggestWin = entry
        }
      } else if (entry.realized_pnl < 0) {
        losingMints++
        if (!biggestLoss || entry.realized_pnl < biggestLoss.realized_pnl) {
          biggestLoss = entry
        }
      }
    }
    const totalMintsWithRealized = winningMints + losingMints
    const winRate =
      totalMintsWithRealized > 0 ? (winningMints / totalMintsWithRealized) * 100 : null
    return { winningMints, losingMints, biggestWin, biggestLoss, winRate }
  }, [pnl])

  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header />
      <main className="px-4 sm:px-6 lg:px-8 pb-16">
        <div className="max-w-5xl mx-auto">
          {!publicKey ? (
            <ConnectPrompt />
          ) : (
            <>
              <div className="mb-8 pt-2">
                <h1
                  className="text-2xl sm:text-3xl font-bold tracking-tight lowercase"
                  style={{ color: 'var(--foreground)' }}
                >
                  lifetime stats
                </h1>
                <p className="text-sm mt-1 font-mono" style={{ color: 'var(--muted)' }}>
                  {shortenAddress(publicKey.toString())}
                </p>
              </div>

              {pnlLoading ? (
                <p className="text-sm" style={{ color: 'var(--muted)' }}>
                  loading…
                </p>
              ) : !pnl ? (
                <p className="text-sm" style={{ color: 'var(--muted)' }}>
                  no indexer configured — connect to an indexer URL to see lifetime stats.
                </p>
              ) : pnl.by_mint.length === 0 ? (
                <p className="text-sm" style={{ color: 'var(--muted)' }}>
                  no trade history yet.{' '}
                  <Link
                    href="/markets"
                    className="underline underline-offset-4"
                    style={{ color: 'var(--accent)' }}
                  >
                    browse markets
                  </Link>{' '}
                  to get started.
                </p>
              ) : (
                <>
                  {/* Aggregate stats — 4-cell strip on desktop, 2x2 on mobile */}
                  <div className="grid grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
                    <StatCell
                      label="realized pnl"
                      value={fmtSigned(pnl.total_realized_pnl / LAMPORTS_PER_SOL)}
                      unit="SOL"
                      color={
                        pnl.total_realized_pnl >= 0 ? '#22c55e' : '#ef4444'
                      }
                    />
                    <StatCell
                      label="lifetime volume"
                      value={fmt(pnl.total_volume / LAMPORTS_PER_SOL)}
                      unit="SOL"
                    />
                    <StatCell
                      label="trades"
                      value={String(pnl.total_trade_count)}
                    />
                    {aggregates && aggregates.winRate != null ? (
                      <StatCell
                        label="winning mints"
                        value={`${aggregates.winningMints}/${aggregates.winningMints + aggregates.losingMints}`}
                        unit={`${aggregates.winRate.toFixed(0)}%`}
                      />
                    ) : (
                      <StatCell label="winning mints" value="—" />
                    )}
                  </div>

                  {/* Biggest win / biggest loss callout */}
                  {aggregates && (aggregates.biggestWin || aggregates.biggestLoss) && (
                    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 mb-8">
                      {aggregates.biggestWin && (
                        <BiggestHitCard
                          entry={aggregates.biggestWin}
                          meta={tokenMeta.get(aggregates.biggestWin.mint)}
                          label="biggest win"
                          color="#22c55e"
                        />
                      )}
                      {aggregates.biggestLoss && (
                        <BiggestHitCard
                          entry={aggregates.biggestLoss}
                          meta={tokenMeta.get(aggregates.biggestLoss.mint)}
                          label="biggest loss"
                          color="#ef4444"
                        />
                      )}
                    </div>
                  )}

                  {/* Per-mint table */}
                  <h2
                    className="text-sm font-medium lowercase tracking-wide mb-3"
                    style={{ color: 'var(--muted)' }}
                  >
                    by mint
                  </h2>
                  <div
                    className="rounded-lg border overflow-x-auto"
                    style={{ borderColor: 'rgba(255,255,255,0.08)' }}
                  >
                    <table className="w-full text-xs">
                      <thead>
                        <tr
                          className="text-left"
                          style={{ color: 'var(--muted)' }}
                        >
                          <th className="px-3 py-2 font-medium">token</th>
                          <th className="px-3 py-2 font-medium text-right">held</th>
                          <th className="px-3 py-2 font-medium text-right">cost basis</th>
                          <th className="px-3 py-2 font-medium text-right">realized pnl</th>
                          <th className="px-3 py-2 font-medium text-right">volume</th>
                          <th className="px-3 py-2 font-medium text-right">trades</th>
                        </tr>
                      </thead>
                      <tbody>
                        {pnl.by_mint.map((entry) => {
                          const meta = tokenMeta.get(entry.mint)
                          const realizedSol = entry.realized_pnl / LAMPORTS_PER_SOL
                          const volumeSol =
                            (entry.total_buy_volume + entry.total_sell_volume) / LAMPORTS_PER_SOL
                          const costBasisSol = entry.cost_basis_remaining / LAMPORTS_PER_SOL
                          return (
                            <tr
                              key={entry.mint}
                              className="border-t"
                              style={{ borderColor: 'rgba(255,255,255,0.04)' }}
                            >
                              <td className="px-3 py-2">
                                <Link
                                  href={`/markets/${entry.mint}`}
                                  className="hover:underline"
                                  style={{ color: 'var(--foreground)' }}
                                >
                                  ${meta?.symbol ?? shortenAddress(entry.mint)}
                                </Link>
                              </td>
                              <td className="px-3 py-2 text-right font-mono">
                                {fmtTokens(entry.tokens_remaining)}
                              </td>
                              <td className="px-3 py-2 text-right font-mono">
                                {fmt(costBasisSol)} SOL
                              </td>
                              <td
                                className="px-3 py-2 text-right font-mono"
                                style={{
                                  color: realizedSol >= 0 ? '#22c55e' : '#ef4444',
                                }}
                              >
                                {fmtSigned(realizedSol)} SOL
                              </td>
                              <td className="px-3 py-2 text-right font-mono">
                                {fmt(volumeSol)} SOL
                              </td>
                              <td className="px-3 py-2 text-right font-mono">
                                {entry.trade_count}
                              </td>
                            </tr>
                          )
                        })}
                      </tbody>
                    </table>
                  </div>
                </>
              )}
            </>
          )}
        </div>
      </main>
    </div>
  )
}

function StatCell({
  label,
  value,
  unit,
  color = 'var(--foreground)',
}: {
  label: string
  value: string
  unit?: string
  color?: string
}) {
  return (
    <div>
      <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
        {label}
      </p>
      <p className="text-xl sm:text-2xl font-mono font-bold mt-0.5" style={{ color }}>
        {value}{' '}
        {unit && (
          <span className="text-sm font-normal" style={{ color: 'var(--muted)' }}>
            {unit}
          </span>
        )}
      </p>
    </div>
  )
}

function BiggestHitCard({
  entry,
  meta,
  label,
  color,
}: {
  entry: UserPnlByMint
  meta?: { name: string; symbol: string }
  label: string
  color: string
}) {
  const realizedSol = entry.realized_pnl / LAMPORTS_PER_SOL
  return (
    <Link
      href={`/markets/${entry.mint}`}
      className="rounded-lg border p-4 block transition-colors hover:bg-white/5"
      style={{ borderColor: 'rgba(255,255,255,0.08)' }}
    >
      <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
        {label}
      </p>
      <p
        className="text-lg sm:text-xl font-mono font-bold mt-0.5"
        style={{ color }}
      >
        {fmtSigned(realizedSol)} SOL
      </p>
      <p className="text-xs mt-0.5" style={{ color: 'var(--muted)' }}>
        ${meta?.symbol ?? shortenAddress(entry.mint)}
        {entry.trade_count > 0 && ` · ${entry.trade_count} trades`}
      </p>
    </Link>
  )
}

function ConnectPrompt() {
  return (
    <div className="min-h-[70vh] flex flex-col items-center justify-center text-center px-6">
      <h1
        className="text-3xl sm:text-4xl font-bold tracking-tight lowercase mb-2"
        style={{ color: 'var(--foreground)' }}
      >
        your stats, on chain.
      </h1>
      <p className="text-base mb-8" style={{ color: 'var(--muted)' }}>
        connect to see lifetime realized pnl, biggest hits, and per-token breakdown.
      </p>
      <WalletMultiButton />
    </div>
  )
}
