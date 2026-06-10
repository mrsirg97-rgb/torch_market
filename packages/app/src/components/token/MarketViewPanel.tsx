'use client'

/**
 * MarketViewPanel — unified market view for a single mint.
 *
 * Layout:
 *   - Top strip (always visible, one row): image · name · ticker · status · price · More ▾
 *   - Disclosure (collapsed by default): description · creator · stars · socials · CA
 *   - Middle: ChartTabs (chart / messages / trades / holders / bubbles / liquidatable)
 *   - Bottom strip: supply + treasury split, with treasury cranks
 *
 * This replaces the old sprawling header card + ChartTabs split on the mint page.
 */
import { useState } from 'react'
import Image from 'next/image'
import { PublicKey } from '@solana/web3.js'
import { ChartTabs, type ChartTabsMessage } from './ChartTabs'
import { VerifiedBadge } from '@/components'
import { LAMPORTS_PER_SOL, shortenAddress } from '@/lib/constants'
import type { UseTokenResult } from '@/hooks/useToken'

// Inline helpers (lifted from page.tsx — moved here as part of the
// MarketViewPanel extraction so the page can shed these locals).
function formatTokenAmount(value: number): string {
  if (value >= 1_000_000_000) return (value / 1_000_000_000).toFixed(2) + 'B'
  if (value >= 1_000_000) return (value / 1_000_000).toFixed(2) + 'M'
  if (value >= 1_000) return (value / 1_000).toFixed(2) + 'K'
  return value.toFixed(2)
}

function truncateMiddle(str: string, startChars = 6, endChars = 4): string {
  if (str.length <= startChars + endChars) return str
  return `${str.slice(0, startChars)}…${str.slice(-endChars)}`
}

interface MarketViewPanelProps {
  token: UseTokenResult
  mint: PublicKey
  holdersCount: number | null
  messages: ChartTabsMessage[]
  saidVerifications: Map<string, { verified: boolean; trustTier: 'high' | 'medium' | 'low' | null }>
  copiedMint: boolean
  onCopyMint: () => void
  onCopyCreator: () => void
  copiedDev: boolean
  walletConnected: boolean
  isOwnToken: boolean
  crankMsg: string | null
  crankLoading: 'harvest' | null
  onHarvestCrank: () => void
}

export function MarketViewPanel({
  token,
  mint,
  holdersCount,
  messages,
  saidVerifications,
  copiedMint,
  onCopyMint,
  onCopyCreator,
  copiedDev,
  walletConnected,
  isOwnToken,
  crankMsg,
  crankLoading,
  onHarvestCrank,
}: MarketViewPanelProps) {
  const [moreOpen, setMoreOpen] = useState(false)

  const statusLabel = token.isMigrated
    ? 'Migrated'
    : token.isComplete || token.isVoting
      ? 'Complete'
      : 'Bonding'
  const statusBadge = token.isMigrated
    ? 'badge-migrated'
    : token.isComplete || token.isVoting
      ? 'badge-complete'
      : 'badge-bonding'

  const priceDisplay =
    token.priceInSol < 0.000001 ? token.priceInSol.toExponential(2) : token.priceInSol.toFixed(8)
  const marketCapUsd =
    (Number(token.marketCapLamports) / LAMPORTS_PER_SOL) * (token.solPriceUsd || 0)

  return (
    <div className="card p-3 flex flex-col gap-3">
      {/* ─── Top strip ──────────────────────────────────────────────────── */}
      <div className="flex items-center gap-3 flex-wrap">
        {/* Image */}
        {token.metadata?.image ? (
          <Image
            src={token.metadata.image}
            alt={token.name}
            width={36}
            height={36}
            className="w-9 h-9 rounded-lg object-cover flex-shrink-0"
            unoptimized
          />
        ) : (
          <div className="w-9 h-9 rounded-lg bg-gradient-to-br from-accent/20 to-danger/20 flex items-center justify-center text-base flex-shrink-0">
            {token.symbol.charAt(0)}
          </div>
        )}

        {/* Name + ticker + status */}
        <div className="flex items-center gap-2 min-w-0">
          <h1 className="text-lg sm:text-xl font-bold text-white truncate">{token.name}</h1>
          <span className="text-white/50 text-sm font-medium">${token.symbol}</span>
          <span className={`badge text-[10px] ${statusBadge}`}>{statusLabel}</span>
        </div>

        {/* Spacer */}
        <div className="flex-1" />

        {/* Price + market cap */}
        <div className="text-right">
          <p className="text-white font-mono text-base sm:text-lg font-bold whitespace-nowrap">
            {priceDisplay} <span className="text-white/40 text-xs">SOL</span>
          </p>
          {token.solPriceUsd ? (
            <p className="text-white/50 font-mono text-xs">
              ${marketCapUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })} mcap
            </p>
          ) : null}
        </div>

        {/* More disclosure toggle */}
        <button
          onClick={() => setMoreOpen((v) => !v)}
          className="text-white/50 hover:text-white text-xs flex items-center gap-1 px-2 py-1 rounded cursor-pointer"
          aria-expanded={moreOpen}
        >
          More
          <svg
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            style={{ transform: moreOpen ? 'rotate(180deg)' : 'rotate(0)', transition: 'transform 0.2s' }}
          >
            <polyline points="6 9 12 15 18 9" />
          </svg>
        </button>
      </div>

      {/* ─── Disclosure (description, creator, stars, socials, CA) ──────── */}
      {moreOpen && (
        <div className="rounded-lg bg-white/[0.03] p-3 space-y-2 text-xs">
          {token.metadata?.description && (
            <p className="text-white/60 leading-relaxed">{token.metadata.description}</p>
          )}

          <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5">
            {/* CA */}
            <button
              onClick={onCopyMint}
              className="flex items-center gap-1 text-white/60 hover:text-white font-mono transition-colors group cursor-pointer"
              title="Copy contract address"
            >
              <span className="text-white/40">CA:</span>
              <span>{truncateMiddle(token.mintAddress, 6, 4)}</span>
              <CopyIcon copied={copiedMint} />
            </button>

            {/* Creator */}
            <button
              onClick={onCopyCreator}
              className="flex items-center gap-1 text-white/60 hover:text-white font-mono transition-colors cursor-pointer"
              title="Copy creator address"
            >
              <span className="text-white/40">Creator:</span>
              <span>{shortenAddress(token.creator)}</span>
              <VerifiedBadge wallet={token.creator} />
              <CopyIcon copied={copiedDev} />
            </button>

            {/* Socials */}
            {token.metadata?.twitter && (
              <a
                href={token.metadata.twitter}
                target="_blank"
                rel="noopener noreferrer"
                className="text-white/40 hover:text-white transition-colors"
                title="Twitter"
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
                </svg>
              </a>
            )}
            {token.metadata?.telegram && (
              <a
                href={token.metadata.telegram}
                target="_blank"
                rel="noopener noreferrer"
                className="text-white/40 hover:text-white transition-colors"
                title="Telegram"
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M11.944 0A12 12 0 000 12a12 12 0 0012 12 12 12 0 0012-12A12 12 0 0012 0a12 12 0 00-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 01.171.325c.016.093.036.306.02.472-.18 1.898-.96 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z" />
                </svg>
              </a>
            )}
            {token.metadata?.website && (
              <a
                href={token.metadata.website}
                target="_blank"
                rel="noopener noreferrer"
                className="text-white/40 hover:text-white transition-colors"
                title="Website"
              >
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <circle cx="12" cy="12" r="10" />
                  <line x1="2" y1="12" x2="22" y2="12" />
                  <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
                </svg>
              </a>
            )}
          </div>
        </div>
      )}

      {/* ─── Middle: chart + view selector ──────────────────────────────── */}
      <div className="lg:h-[560px]">
        <ChartTabs
          mint={mint}
          priceInSol={token.priceInSol}
          solRaised={token.solRaised}
          solPriceUsd={token.solPriceUsd}
          priceHistory={token.priceHistory}
          messages={messages}
          saidVerifications={saidVerifications}
        />
      </div>

      {/* ─── Bottom strip: supply + treasury ────────────────────────────── */}
      <div className="rounded-lg bg-white/[0.03] p-3 flex flex-col sm:flex-row gap-4">
        {/* Supply */}
        <div className="flex-1">
          <div className="flex items-center gap-2 mb-2">
            <h3 className="text-white/50 text-xs font-medium lowercase tracking-wide">Supply</h3>
            {holdersCount !== null && (
              <span className="text-white/40 text-[10px] bg-white/10 px-1.5 py-0.5 rounded">
                {holdersCount} holders
              </span>
            )}
          </div>
          {token.isMigrated || token.isComplete ? (
            <div className="space-y-1.5 text-xs">
              <div className="flex items-center gap-2 text-success text-xs">
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
                  <polyline points="22 4 12 14.01 9 11.01" />
                </svg>
                <span className="font-medium">Graduated</span>
                <span className="text-white/40">Raised {token.solRaised.toFixed(2)} SOL</span>
              </div>
              {token.treasuryLockBalance > 0 && (
                <div className="flex justify-between">
                  <span className="text-white/40">Locked in Treasury</span>
                  <span className="text-accent font-mono">
                    {formatTokenAmount(token.treasuryLockBalance)}
                  </span>
                </div>
              )}
              <div className="flex justify-between">
                <span className="text-white/40">In DEX Pool</span>
                <span className="text-success font-mono">
                  {formatTokenAmount(
                    token.poolTokenBalance > 0 ? token.poolTokenBalance : token.tokensInCurve,
                  )}
                </span>
              </div>
            </div>
          ) : (
            <div className="space-y-2">
              <div>
                <div className="flex justify-between text-xs text-white/50 mb-1">
                  <span>
                    {token.solRaised.toFixed(2)} / {token.solTarget} SOL
                  </span>
                  <span>{token.progress.toFixed(1)}%</span>
                </div>
                <div className="progress-bar h-2">
                  <div
                    className="progress-bar-fill"
                    style={{ width: `${Math.min(token.progress, 100)}%` }}
                  />
                </div>
              </div>
              <div className="space-y-1 text-xs">
                {token.treasuryLockBalance > 0 && (
                  <div className="flex justify-between">
                    <span className="text-white/40">Locked in Treasury</span>
                    <span className="text-accent font-mono">
                      {formatTokenAmount(token.treasuryLockBalance)}
                    </span>
                  </div>
                )}
                <div className="flex justify-between">
                  <span className="text-white/40">Available</span>
                  <span className="text-white font-mono">
                    {formatTokenAmount(token.tokensInCurve)}
                  </span>
                </div>
              </div>
            </div>
          )}

          {/* Treasury crank — permissionless harvest+swap. Lives under
              Supply because it's a community/economy action concerning
              token supply flows, not a wallet concern. */}
          {token.isMigrated && walletConnected && (
            <div className="pt-2 mt-2 border-t border-white/10 space-y-1.5">
              {crankMsg && (
                <p
                  className={`text-xs ${
                    crankMsg.startsWith('Error') ? 'text-danger' : 'text-success'
                  }`}
                >
                  {crankMsg}
                </p>
              )}
              <button
                onClick={onHarvestCrank}
                disabled={crankLoading !== null}
                className="w-full py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/10 text-white/70 hover:bg-white/20"
              >
                {crankLoading === 'harvest' ? 'Swapping…' : 'Harvest & Swap'}
              </button>
              <p className="text-white/30 text-[10px]">Permissionless — anyone can trigger</p>
            </div>
          )}
        </div>

        {/* Treasury */}
        <div className="flex-1 sm:border-l sm:border-white/5 sm:pl-4">
          <h3 className="text-white/50 text-xs font-medium mb-2 lowercase tracking-wide">
            Treasury
          </h3>
          <div className="space-y-1 text-xs">
            <div className="flex justify-between">
              <span className="text-white/40">SOL Balance</span>
              <span className="text-white font-mono">
                {token.treasurySolBalance.toFixed(4)} SOL
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-white/40">Balance ({token.symbol})</span>
              <span className="text-white font-mono">
                {formatTokenAmount(token.treasuryTokenBalance)}
              </span>
            </div>
            {token.isMigrated && token.mintWithheldFees > 0 && (
              <div className="flex justify-between">
                <span className="text-white/40">Harvestable Fees</span>
                <span className="text-accent font-mono">
                  {formatTokenAmount(token.mintWithheldFees)}
                </span>
              </div>
            )}
          </div>

          {/* SOL distribution bar (migrated only) */}
          {token.isMigrated && token.treasurySolBalance > 0 && (() => {
            // [F-2] Principal pool = physical float + lent (the float is
            // already net of lent SOL). Available = the physical float —
            // fully lendable, first-come-first-serve.
            const lent = token.totalSolLent
            const available = token.treasurySolBalance
            const total = available + lent
            const reservePct = token.reserveRatioBps / 100
            const reserve = (total * reservePct) / 100

            const lentPct = total > 0 ? (lent / total) * 100 : 0
            const availPct = total > 0 ? (available / total) * 100 : 0
            const reserveBarPct = total > 0 ? (reserve / total) * 100 : 0

            return (
              <div className="mt-2 pt-2 border-t border-white/10 space-y-1.5">
                <p className="text-white/40 text-[10px] lowercase tracking-wide">
                  SOL Distribution
                </p>
                <div className="h-1.5 bg-white/5 rounded-full overflow-hidden flex">
                  {lentPct > 0 && (
                    <div
                      className="bg-blue-500 h-full"
                      style={{ width: `${lentPct}%` }}
                      title={`Lent: ${lent.toFixed(2)} SOL`}
                    />
                  )}
                  {availPct > 0 && (
                    <div
                      className="bg-green-500/60 h-full"
                      style={{ width: `${availPct}%` }}
                      title={`Available: ${available.toFixed(2)} SOL`}
                    />
                  )}
                  {reserveBarPct > 0 && (
                    <div
                      className="bg-white/5 h-full"
                      style={{ width: `${reserveBarPct}%` }}
                      title={`Reserve: ${reserve.toFixed(2)} SOL`}
                    />
                  )}
                </div>
                <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-[10px]">
                  {lent > 0 && (
                    <span className="flex items-center gap-1">
                      <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />
                      <span className="text-white/40">Lent {lent.toFixed(2)}</span>
                    </span>
                  )}
                  <span className="flex items-center gap-1">
                    <span className="w-1.5 h-1.5 rounded-full bg-green-500/60" />
                    <span className="text-white/40">Lendable {available.toFixed(2)}</span>
                  </span>
                  <span className="flex items-center gap-1">
                    <span className="w-1.5 h-1.5 rounded-full bg-white/5 border border-white/10" />
                    <span className="text-white/40">Reserve {reserve.toFixed(2)}</span>
                  </span>
                </div>
              </div>
            )
          })()}
        </div>
      </div>
    </div>
  )
}

function CopyIcon({ copied }: { copied: boolean }) {
  if (copied) {
    return (
      <svg
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
    )
  }
  return (
    <svg
      width="10"
      height="10"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="opacity-40 group-hover:opacity-80"
    >
      <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  )
}
