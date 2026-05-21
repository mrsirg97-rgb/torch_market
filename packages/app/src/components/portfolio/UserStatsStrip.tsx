'use client'

import { useUserStats } from '@/hooks/useUserStats'

interface UserStatsStripProps {
  /** Sum of (balance / decimals × price_sol) across all held markets. */
  portfolioValueSol: number
}

function fmt(n: number): string {
  if (n >= 1000) return n.toFixed(0)
  if (n >= 1) return n.toFixed(2)
  if (n >= 0.001) return n.toFixed(4)
  if (n > 0) return n.toExponential(2)
  return '0'
}

/**
 * Top-of-portfolio aggregate strip. Three cells:
 *   - Portfolio value (current price × balances, summed)
 *   - Total volume (lifetime, from UserStats)
 *   - Current epoch volume (claim-eligibility proxy)
 */
export function UserStatsStrip({ portfolioValueSol }: UserStatsStripProps) {
  const { stats, loading } = useUserStats()

  const totalVolume = stats?.total_volume_sol ?? 0
  const currentEpochVolume = stats?.volume_current_epoch_sol ?? 0

  return (
    <div className="grid grid-cols-3 gap-4 mb-8">
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
          lifetime volume
        </p>
        <p
          className="text-xl sm:text-2xl font-mono font-bold mt-0.5"
          style={{ color: 'var(--foreground)' }}
        >
          {loading ? '…' : fmt(totalVolume)}{' '}
          <span className="text-sm font-normal" style={{ color: 'var(--muted)' }}>
            SOL
          </span>
        </p>
      </div>
      <div>
        <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
          this epoch
        </p>
        <p
          className="text-xl sm:text-2xl font-mono font-bold mt-0.5"
          style={{ color: 'var(--foreground)' }}
        >
          {loading ? '…' : fmt(currentEpochVolume)}{' '}
          <span className="text-sm font-normal" style={{ color: 'var(--muted)' }}>
            SOL
          </span>
        </p>
      </div>
    </div>
  )
}
