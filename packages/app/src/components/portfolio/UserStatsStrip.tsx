'use client'

import { useUserPnl } from '@/hooks/useUserPnl'

interface UserStatsStripProps {
  /** Sum of (balance / decimals × price_sol) across all held markets. */
  portfolioValueSol: number
}

const LAMPORTS_PER_SOL = 1_000_000_000

function fmt(n: number): string {
  if (Math.abs(n) >= 1000) return n.toFixed(0)
  if (Math.abs(n) >= 1) return n.toFixed(2)
  if (Math.abs(n) >= 0.001) return n.toFixed(4)
  if (Math.abs(n) > 0) return n.toExponential(2)
  return '0'
}

/**
 * Top-of-portfolio summary. Two cells:
 *   - Portfolio value (current price × balances, summed)
 *   - Realized PnL (lifetime, FIFO over trades ∪ swaps via indexer)
 *
 * Volume stats (lifetime + current epoch from on-chain UserStats) were
 * removed because UserStats only tracks the user's own wallet — it
 * doesn't see vault-mediated trades. For vault-heavy traders, volume
 * numbers under-counted significantly. Realized PnL covers both paths
 * via the indexer.
 *
 * Layout: 2-column on mobile (side-by-side), 1-column when stacked in the
 * desktop dashboard's left rail. Caller controls width via the wrapper.
 */
export function UserStatsStrip({ portfolioValueSol }: UserStatsStripProps) {
  const { pnl } = useUserPnl()

  const realizedPnlSol = pnl != null ? pnl.total_realized_pnl / LAMPORTS_PER_SOL : null
  const pnlColor =
    realizedPnlSol == null
      ? 'var(--foreground)'
      : realizedPnlSol >= 0
        ? '#22c55e'
        : '#ef4444'

  return (
    <div className="grid grid-cols-2 lg:grid-cols-1 gap-4">
      <div>
        <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
          portfolio value
        </p>
        <p
          className="text-xl sm:text-2xl font-mono font-bold mt-0.5"
          style={{ color: 'var(--foreground)' }}
        >
          {fmt(portfolioValueSol)}{' '}
          <span className="text-sm font-normal" style={{ color: 'var(--muted)' }}>
            SOL
          </span>
        </p>
      </div>
      <div>
        <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
          realized pnl
        </p>
        <p
          className="text-xl sm:text-2xl font-mono font-bold mt-0.5"
          style={{ color: pnlColor }}
        >
          {realizedPnlSol == null ? (
            '—'
          ) : (
            <>
              {realizedPnlSol >= 0 ? '+' : ''}
              {fmt(realizedPnlSol)}{' '}
              <span className="text-sm font-normal" style={{ color: 'var(--muted)' }}>
                SOL
              </span>
            </>
          )}
        </p>
      </div>
    </div>
  )
}
