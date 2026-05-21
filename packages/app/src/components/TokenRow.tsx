'use client'

import { useState } from 'react'
import Link from 'next/link'
import Image from 'next/image'
import { TokenData, getTokenStatus, TIER_CONFIG } from '@/types/token'
import { shortenAddress } from '@/lib/constants'
import { VerifiedBadge } from './VerifiedBadge'

interface TokenRowProps {
  token: TokenData
  currentSlot?: bigint
}

export function TokenRow({ token, currentSlot }: TokenRowProps) {
  const [copied, setCopied] = useState(false)

  const status = getTokenStatus(token)
  const isInactive = status === 'reclaimed' || status === 'legacy'

  const priceDisplay =
    token.price_sol < 0.000001
      ? token.price_sol.toExponential(2)
      : token.price_sol < 0.001
        ? token.price_sol.toFixed(6)
        : token.price_sol.toFixed(4)

  const marketCapDisplay =
    token.market_cap_sol >= 1000
      ? (token.market_cap_sol / 1000).toFixed(1) + 'K'
      : token.market_cap_sol.toFixed(1)

  const statusConfig: Record<string, { label: string; className: string }> = {
    new: { label: 'New', className: 'badge badge-new' },
    bonding: { label: 'Bonding', className: 'badge badge-bonding' },
    complete: { label: 'Complete', className: 'badge badge-complete' },
    migrated: { label: 'Active', className: 'badge badge-migrated' },
    reclaimed: { label: 'Reclaimed', className: 'badge badge-reclaimed' },
    legacy: { label: 'Legacy', className: 'badge badge-reclaimed' },
  }

  const { label: statusLabel, className: statusClass } = statusConfig[status] || statusConfig.bonding

  const isBonding = status === 'new' || status === 'bonding'
  const isActive = status === 'migrated' || status === 'complete'

  return (
    <Link href={`/markets/${token.mint}`}>
      <div
        className={`token-card flex items-center gap-3 sm:gap-4 px-3 sm:px-4 py-3 cursor-pointer ${isInactive ? 'opacity-40 grayscale' : ''}`}
      >
        {/* Avatar */}
        <div className="w-9 h-9 sm:w-10 sm:h-10 rounded-lg overflow-hidden flex-shrink-0 bg-white/5">
          {token.image ? (
            <Image
              src={token.image}
              alt={token.name}
              width={40}
              height={40}
              className="w-full h-full object-cover"
              unoptimized
            />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-white/30 text-sm font-bold">
              {token.symbol.charAt(0)}
            </div>
          )}
        </div>

        {/* Name + Symbol */}
        <div className="min-w-0 w-28 sm:w-36 flex-shrink-0">
          <div className="flex items-center gap-1.5">
            <span className="text-white font-semibold text-sm truncate">{token.name}</span>
            {token.creator && <VerifiedBadge wallet={token.creator} />}
          </div>
          <div className="flex items-center gap-1.5 mt-0.5">
            <span className="text-white/40 text-xs">${token.symbol}</span>
            {token.tier && token.tier !== 'torch' && (
              <span
                className="text-[9px] font-medium px-1 py-0 rounded"
                style={{
                  color: TIER_CONFIG[token.tier].color,
                  backgroundColor: TIER_CONFIG[token.tier].color + '12',
                }}
              >
                {TIER_CONFIG[token.tier].solTarget}
              </span>
            )}
          </div>
        </div>

        {/* Status badge */}
        <div className="flex-shrink-0 hidden sm:block">
          <span className={`${statusClass} text-[11px]`}>{statusLabel}</span>
        </div>

        {/* Middle section - context-dependent */}
        <div className="flex-1 min-w-0">
          {isBonding ? (
            /* Bonding: show progress bar */
            <div className="flex items-center gap-2">
              <div className="progress-bar h-1.5 flex-1 max-w-[140px]">
                <div
                  className="progress-bar-fill"
                  style={{ width: `${Math.min(token.progress_percent, 100)}%` }}
                />
              </div>
              <span className="text-white/40 text-xs font-mono w-10 text-right">
                {token.progress_percent.toFixed(0)}%
              </span>
            </div>
          ) : isActive ? (
            /* Active: show market cap */
            <div className="flex items-center gap-3">
              <div>
                <span className="text-white/30 text-[10px]">MCap</span>
                <span className="text-white/60 text-xs font-mono ml-1.5">{marketCapDisplay} SOL</span>
              </div>
              {token.holders && token.holders > 0 && (
                <div className="hidden lg:block">
                  <span className="text-white/30 text-[10px]">Holders</span>
                  <span className="text-white/60 text-xs font-mono ml-1.5">{token.holders}</span>
                </div>
              )}
            </div>
          ) : null}
        </div>

        {/* Price - always visible */}
        <div className="flex-shrink-0 text-right">
          <p className="text-white/30 text-[10px] hidden sm:block">Price</p>
          <p className="text-white font-mono text-xs">{priceDisplay}</p>
        </div>

        {/* Contract address - desktop only */}
        <div className="flex-shrink-0 hidden lg:block">
          <button
            onClick={(e) => {
              e.preventDefault()
              e.stopPropagation()
              navigator.clipboard.writeText(token.mint)
              setCopied(true)
              setTimeout(() => setCopied(false), 1500)
            }}
            className="text-white/25 hover:text-white/50 text-xs font-mono transition-colors cursor-pointer"
            title="Copy contract address"
          >
            {copied ? (
              <span className="text-[#22c55e]">copied</span>
            ) : (
              `${token.mint.slice(0, 4)}...${token.mint.slice(-4)}`
            )}
          </button>
        </div>

        {/* Chevron */}
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-white/15 flex-shrink-0"
        >
          <polyline points="9 18 15 12 9 6" />
        </svg>
      </div>
    </Link>
  )
}
