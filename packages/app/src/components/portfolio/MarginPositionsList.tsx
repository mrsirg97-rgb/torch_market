'use client'

import Link from 'next/link'
import { TokenData } from '@/types/token'
import { MarginPositionEntry } from '@/hooks/useMarginPositions'
import { LAMPORTS_PER_SOL, TOKEN_MULTIPLIER } from '@/lib/constants'

interface MarginPositionsListProps {
  positions: MarginPositionEntry[]
  tokens: TokenData[]
  loading: boolean
}

const healthColor = (h: string | undefined) => {
  if (h === 'liquidatable') return 'var(--danger)'
  if (h === 'at_risk') return 'var(--accent)'
  return 'var(--success)'
}

/**
 * Inline list of open margin positions (loans + shorts) for the connected
 * wallet, scoped to markets the wallet currently holds. Renders nothing
 * when there are no open positions.
 */
export function MarginPositionsList({ positions, tokens, loading }: MarginPositionsListProps) {
  if (loading && positions.length === 0) {
    return (
      <p className="text-sm py-6" style={{ color: 'var(--muted)' }}>
        loading positions…
      </p>
    )
  }

  if (positions.length === 0) return null

  const tokenByMint = new Map(tokens.map((t) => [t.mint, t]))

  return (
    <div className="flex flex-col gap-2">
      {positions.map((p) => {
        const token = tokenByMint.get(p.mint)
        if (!token) return null
        return (
          <Link
            href={`/markets/${p.mint}`}
            key={p.mint}
            className="token-card p-3 cursor-pointer"
          >
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p
                  className="text-sm font-semibold truncate"
                  style={{ color: 'var(--foreground)' }}
                >
                  {token.name}{' '}
                  <span className="font-normal" style={{ color: 'var(--muted)' }}>
                    ${token.symbol}
                  </span>
                </p>
                <div className="flex items-center gap-3 mt-1 text-xs">
                  {p.loan && (
                    <span style={{ color: healthColor(p.loan.health) }}>
                      borrow · {(p.loan.total_owed / LAMPORTS_PER_SOL).toFixed(3)} SOL
                    </span>
                  )}
                  {p.short && (
                    <span style={{ color: healthColor(p.short.health) }}>
                      short · {(p.short.debt_amount / TOKEN_MULTIPLIER).toFixed(0)} ${token.symbol}
                    </span>
                  )}
                </div>
              </div>
              <span className="text-xs" style={{ color: 'var(--muted)' }}>
                manage →
              </span>
            </div>
          </Link>
        )
      })}
    </div>
  )
}
