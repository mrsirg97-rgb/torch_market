'use client'

import { useState } from 'react'
import Link from 'next/link'
import Image from 'next/image'
import { TokenData, getTokenStatus } from '@/types/token'
import { formatTokens } from '@/lib/constants'

interface TokenCardPortfolioProps {
  token: TokenData
  onClick?: () => void
  /** Total balance held by the user across wallet + vault. */
  userBalance?: bigint
  /** Balance in the wallet ATA alone. Optional — when provided alongside
   *  `vaultBalance`, the card renders a small "vault" or "wallet+vault" chip. */
  walletBalance?: bigint
  /** Balance in the vault ATA alone. See `walletBalance`. */
  vaultBalance?: bigint
  /** Current SOL value of the position. When provided, shown as the primary stat. */
  valueSol?: number
  /** This position's share of the total portfolio (0–100). Renders next to value. */
  portfolioPct?: number
  /** Unrealized PnL in SOL: current value − cost basis of remaining tokens.
   *  Positive = green +X, negative = red −X. Requires indexer-served PnL data
   *  to be available; omit when unknown. */
  unrealizedPnlSol?: number
}

function fmtValue(n: number): string {
  if (n >= 1) return n.toFixed(2)
  if (n >= 0.001) return n.toFixed(4)
  if (n > 0) return n.toExponential(2)
  return '0'
}

/**
 * Condensed token card for portfolio view
 * Shows: name, ticker with copy button, status badge, and optionally user holdings
 */
export function TokenCardPortfolio({
  token,
  onClick,
  userBalance,
  walletBalance,
  vaultBalance,
  valueSol,
  portfolioPct,
  unrealizedPnlSol,
}: TokenCardPortfolioProps) {
  const [copied, setCopied] = useState(false)

  // Copy handler
  const handleCopy = (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    navigator.clipboard.writeText(token.mint)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const status = getTokenStatus(token)

  const statusBadge = {
    new: { class: 'badge-new', text: 'New' },
    bonding: { class: 'badge-bonding', text: 'Bonding' },
    complete: { class: 'badge-complete', text: 'Complete' },
    migrated: { class: 'badge-migrated', text: 'Migrated' },
    reclaimed: { class: 'badge-reclaimed', text: 'Reclaimed' },
    legacy: { class: 'badge-reclaimed', text: 'Legacy' },
  }[status]

  const isReclaimed = status === 'reclaimed'
  const isLegacy = status === 'legacy'
  const isInactive = isReclaimed || isLegacy

  return (
    <Link
      href={`/markets/${token.mint}`}
      onClick={onClick}
      className={`block ${isReclaimed ? 'pointer-events-none' : ''}`}
    >
      <div
        className={`token-card p-3 ${isInactive ? 'opacity-50 grayscale cursor-default' : 'cursor-pointer'}`}
      >
        <div className="flex items-center gap-3">
          {/* Token Image */}
          <div
            className="w-10 h-10 rounded-lg flex items-center justify-center text-sm overflow-hidden flex-shrink-0"
            style={{
              background:
                'linear-gradient(135deg, color-mix(in srgb, var(--accent) 18%, transparent), color-mix(in srgb, var(--secondary) 22%, transparent))',
            }}
          >
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
              <span className="text-white/50">{token.symbol.charAt(0)}</span>
            )}
          </div>

          {/* Token Info */}
          <div className="flex-1 min-w-0">
            {/* Row 1: Name + Status Badge */}
            <div className="flex items-center justify-between gap-2">
              <span className="text-white font-medium text-sm truncate">{token.name}</span>
              <span className={`badge ${statusBadge.class} text-xs flex-shrink-0`}>
                {statusBadge.text}
              </span>
            </div>

            {/* Row 2: Ticker with copy button */}
            <div className="flex items-center justify-between mt-1">
              <button
                onClick={handleCopy}
                className="flex items-center gap-1 text-white/40 hover:text-white/70 transition-colors cursor-pointer"
                title="Copy contract address"
              >
                <span className="text-xs">${token.symbol}</span>
                <span className="text-white/30 mx-1">·</span>
                <span className="text-xs font-mono">
                  {token.mint.slice(0, 4)}...{token.mint.slice(-4)}
                </span>
                {copied ? (
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    width="10"
                    height="10"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    className="text-success"
                  >
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                ) : (
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    width="10"
                    height="10"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  >
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                  </svg>
                )}
              </button>

              {/* User Holdings (when provided) + optional source chip + value */}
              {userBalance !== undefined && userBalance > BigInt(0) && (
                <div className="flex items-center gap-2 text-xs">
                  {(() => {
                    const hasWallet = walletBalance !== undefined && walletBalance > BigInt(0)
                    const hasVault = vaultBalance !== undefined && vaultBalance > BigInt(0)
                    if (!hasVault) return null
                    return (
                      <span
                        className="text-[10px] px-1.5 py-0.5 rounded-md font-medium lowercase"
                        style={{
                          background: 'color-mix(in srgb, var(--accent) 14%, transparent)',
                          color: 'var(--accent)',
                        }}
                      >
                        {hasWallet ? 'wallet + vault' : 'vault'}
                      </span>
                    )
                  })()}
                  <div className="flex flex-col items-end leading-tight">
                    {valueSol !== undefined && valueSol > 0 ? (
                      <>
                        <span className="font-mono text-sm" style={{ color: 'var(--foreground)' }}>
                          {fmtValue(valueSol)} SOL
                        </span>
                        {unrealizedPnlSol !== undefined && (
                          <span
                            className="font-mono text-[10px]"
                            style={{
                              color: unrealizedPnlSol >= 0 ? '#22c55e' : '#ef4444',
                            }}
                          >
                            {unrealizedPnlSol >= 0 ? '+' : ''}
                            {fmtValue(Math.abs(unrealizedPnlSol))} SOL
                          </span>
                        )}
                        <span className="font-mono text-[10px]" style={{ color: 'var(--muted)' }}>
                          {formatTokens(userBalance)}
                          {portfolioPct !== undefined && portfolioPct > 0 && (
                            <> · {portfolioPct.toFixed(1)}%</>
                          )}
                        </span>
                      </>
                    ) : (
                      <span className="text-white font-mono">{formatTokens(userBalance)}</span>
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </Link>
  )
}
