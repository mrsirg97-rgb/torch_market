'use client'

import { useMemo } from 'react'
import { TokenData, getTokenStatus } from '@/types/token'

interface PortfolioStatsProps {
  tokens: TokenData[]
}

/**
 * Top stats component for portfolio showing launch metrics
 *
 * V10 Note: Creator rewards are now per-token (stars on Treasury account)
 * with automatic payout at 2000 stars. No more per-creator stats to display.
 */
export function PortfolioStats({ tokens }: PortfolioStatsProps) {
  const stats = useMemo(() => {
    const totalLaunched = tokens.length
    let migrated = 0
    let bonding = 0
    let complete = 0

    tokens.forEach((t) => {
      const status = getTokenStatus(t)
      if (status === 'migrated') migrated++
      else if (status === 'complete') complete++
      else if (status === 'bonding' || status === 'new') bonding++
    })

    return { totalLaunched, migrated, bonding, complete }
  }, [tokens])

  if (stats.totalLaunched === 0) {
    return null
  }

  return (
    <div className="grid grid-cols-4 gap-3 mb-6">
      <div>
        <p className="text-2xl font-bold" style={{ color: 'var(--foreground)' }}>
          {stats.totalLaunched}
        </p>
        <p className="text-xs" style={{ color: 'var(--muted)' }}>positions</p>
      </div>
      <div>
        <p className="text-2xl font-bold text-success">{stats.migrated}</p>
        <p className="text-xs" style={{ color: 'var(--muted)' }}>migrated</p>
      </div>
      <div>
        <p className="text-2xl font-bold text-accent">{stats.bonding + stats.complete}</p>
        <p className="text-xs" style={{ color: 'var(--muted)' }}>active</p>
      </div>
      <div>
        <p className="text-2xl font-bold" style={{ color: 'var(--muted)' }}>0</p>
        <p className="text-xs" style={{ color: 'var(--muted)' }}>reclaimed</p>
      </div>
    </div>
  )
}
