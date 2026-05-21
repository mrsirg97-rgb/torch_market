'use client'

import Link from 'next/link'
import Image from 'next/image'
import { TokenData, TIER_CONFIG, getTierFromTarget } from '@/types/token'

interface TokenCardCompactProps {
  token: TokenData
  variant?: 'top' | 'trending'
}

/**
 * Compact token card for Top Projects and Trending carousels
 */
export function TokenCardCompact({ token, variant = 'top' }: TokenCardCompactProps) {
  const tier = token.tier || getTierFromTarget(token.bonding_target || 0)
  const tierConfig = TIER_CONFIG[tier]
  const gradient =
    variant === 'top'
      ? 'linear-gradient(135deg, color-mix(in srgb, var(--accent) 20%, transparent), color-mix(in srgb, var(--secondary) 22%, transparent))'
      : 'linear-gradient(135deg, color-mix(in srgb, var(--accent) 22%, transparent), color-mix(in srgb, var(--danger) 20%, transparent))'

  return (
    <Link
      href={`/markets/${token.mint}`}
      className="flex items-center gap-3 rounded-lg p-2 -m-2 transition-colors"
      style={{ background: 'transparent' }}
      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--surface-hover)')}
      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
    >
      <div
        className="w-12 h-12 rounded-lg flex items-center justify-center text-base overflow-hidden flex-shrink-0"
        style={{ background: gradient }}
      >
        {token.image ? (
          <Image
            src={token.image}
            alt={token.name}
            width={48}
            height={48}
            className="w-full h-full object-cover"
            unoptimized
          />
        ) : (
          token.symbol.charAt(0)
        )}
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-white font-medium text-sm truncate">{token.name}</span>
          <span className="text-white/40 text-xs">${token.symbol}</span>
        </div>
        <div className="flex items-center gap-2 mt-0.5">
          {variant === 'top' ? (
            <span className="text-white/50 text-xs">
              {token.market_cap_sol.toFixed(1)} SOL mcap
            </span>
          ) : (
            <span className="text-white/50 text-xs">
              {token.progress_percent.toFixed(0)}% bonded
            </span>
          )}
          <span
            className="text-xs font-medium"
            style={{ color: tierConfig.color }}
          >
            {tierConfig.label}
          </span>
        </div>
        <p className="text-white/30 text-xs font-mono truncate mt-0.5">{token.mint}</p>
      </div>
    </Link>
  )
}
