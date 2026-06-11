'use client'

import { useState } from 'react'
import Link from 'next/link'
import Image from 'next/image'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { buildReclaimFailedTokenTransaction } from 'torchsdk'
import { TokenData, getTokenStatus, TIER_CONFIG } from '@/types/token'
import { shortenAddress, formatSlotAge, INACTIVITY_PERIOD_SLOTS } from '@/lib/constants'
import { VerifiedBadge } from './VerifiedBadge'

interface TokenCardProps {
  token: TokenData
  currentSlot?: bigint
  onClick?: () => void
}

export function TokenCard({ token, currentSlot, onClick }: TokenCardProps) {
  const [copied, setCopied] = useState(false)
  const [creatorCopied, setCreatorCopied] = useState(false)
  const [reclaiming, setReclaiming] = useState(false)
  const [reclaimResult, setReclaimResult] = useState<string | null>(null)
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const handleCopy = (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    navigator.clipboard.writeText(token.mint)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const status = getTokenStatus(token)

  // Reclaim eligibility: bonding (not complete/migrated/reclaimed), inactive 7+ days
  const isReclaimEligible = status !== 'reclaimed' && status !== 'migrated' && status !== 'complete'
    && currentSlot && token.last_activity_at
    && (currentSlot - BigInt(token.last_activity_at)) >= INACTIVITY_PERIOD_SLOTS

  const handleReclaim = async (e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    if (!publicKey || reclaiming) return

    setReclaiming(true)
    setReclaimResult(null)
    try {
      const { transaction } = await buildReclaimFailedTokenTransaction(connection, {
        payer: publicKey.toString(),
        mint: token.mint,
      })
      const txId = await sendTransaction(transaction)
      const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash()
      await connection.confirmTransaction({ signature: txId, blockhash, lastValidBlockHeight }, 'confirmed')
      setReclaimResult('Reclaimed!')
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed'
      setReclaimResult(msg.includes('User rejected') ? 'Cancelled' : 'Error')
    } finally {
      setReclaiming(false)
      setTimeout(() => setReclaimResult(null), 3000)
    }
  }

  const statusBadge = {
    new: { class: 'badge-new', text: 'New' },
    bonding: { class: 'badge-bonding', text: 'Bonding' },
    complete: { class: 'badge-complete', text: 'Complete' },
    migrated: { class: 'badge-migrated', text: 'Migrated' },
    reclaimed: { class: 'badge-reclaimed', text: 'Reclaimed' },
  }[status]

  const isReclaimed = status === 'reclaimed'
  const isInactive = isReclaimed

  // Format price for display
  const priceDisplay =
    token.price_sol < 0.000001
      ? token.price_sol.toExponential(2)
      : token.price_sol.toFixed(6)

  // Format market cap
  const marketCapDisplay =
    token.market_cap_sol >= 1000
      ? (token.market_cap_sol / 1000).toFixed(1) + 'K'
      : token.market_cap_sol.toFixed(2)

  return (
    <Link
      href={`/markets/${token.mint}`}
      onClick={onClick}
      className=""
    >
      <div
        className={`token-card-v2 ${isInactive ? 'opacity-50 grayscale cursor-default' : 'cursor-pointer'}`}
      >
        {/* Main 2-column layout: 25% image / 75% content */}
        <div className="flex h-full">
          {/* Left column - Token Image (25% width, full height) */}
          <div
            className="w-1/4 flex-shrink-0 flex items-center justify-center overflow-hidden rounded-l-[14px]"
            style={{
              background:
                'linear-gradient(135deg, color-mix(in srgb, var(--accent) 18%, transparent), color-mix(in srgb, var(--secondary) 22%, transparent))',
            }}
          >
            {token.image ? (
              <Image
                src={token.image}
                alt={token.name}
                width={160}
                height={160}
                className="w-full h-full object-cover"
                unoptimized
              />
            ) : (
              <span className="text-white/50 text-4xl">{token.symbol.charAt(0)}</span>
            )}
          </div>

          {/* Right column - Content (75% width) */}
          <div className="flex-1 min-w-0 p-4 flex flex-col">
            {/* Row 1: Name + Verified + Ticker + Status Badge */}
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-1 min-w-0">
                <h3 className="font-bold text-lg text-white truncate">{token.name}</h3>
              </div>
              <p className="text-white/50 text-sm font-medium flex-shrink-0">${token.symbol}</p>
              {token.tier && token.tier !== 'torch' && (
                <span
                  className="text-[10px] font-medium px-1.5 py-0.5 rounded flex-shrink-0"
                  style={{
                    color: TIER_CONFIG[token.tier].color,
                    backgroundColor: TIER_CONFIG[token.tier].color + '15',
                    border: `1px solid ${TIER_CONFIG[token.tier].color}30`,
                  }}
                >
                  {TIER_CONFIG[token.tier].solTarget} SOL
                </span>
              )}
              <span className={`badge ${statusBadge.class} flex-shrink-0`}>{statusBadge.text}</span>
              {isReclaimEligible && publicKey && (
                <button
                  onClick={handleReclaim}
                  disabled={reclaiming}
                  className="px-2 py-0.5 text-[10px] font-medium rounded bg-red-500/20 text-red-400 hover:bg-red-500/30 transition-colors cursor-pointer disabled:opacity-50 flex-shrink-0"
                >
                  {reclaiming ? '...' : reclaimResult || 'Reclaim'}
                </button>
              )}
            </div>

            {/* Row 2: CA with copy button */}
            <div className="mt-2">
              <button
                onClick={handleCopy}
                className="inline-flex items-center gap-1.5 text-white/40 hover:text-white/70 transition-colors cursor-pointer"
                title="Copy contract address"
              >
                <span className="text-xs font-mono">
                  {token.mint.slice(0, 6)}...{token.mint.slice(-6)}
                </span>
                {copied ? (
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
                    className="text-success"
                  >
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                ) : (
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
                  >
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                  </svg>
                )}
              </button>
            </div>

            {/* Stats Row: Price | Progress | Market Cap */}
            <div className="flex items-center gap-3 text-sm mt-auto pt-3">
              {/* Price */}
              <div className="flex-shrink-0">
                <p className="text-white/50 text-[10px]">Price</p>
                <p className="text-white font-mono text-xs">{priceDisplay}</p>
              </div>

              {/* Progress */}
              <div className="flex-1 min-w-0">
                <p className="text-white/50 text-[10px]">Progress</p>
                <div className="flex items-center gap-2">
                  <div className="progress-bar h-1.5 flex-1">
                    <div
                      className="progress-bar-fill"
                      style={{ width: `${Math.min(token.progress_percent, 100)}%` }}
                    />
                  </div>
                  <span className="text-white/50 text-[10px] font-mono">
                    {token.progress_percent.toFixed(0)}%
                  </span>
                </div>
              </div>

              {/* Market Cap */}
              <div className="flex-shrink-0">
                <p className="text-white/50 text-[10px]">MCap</p>
                <p className="text-accent font-mono text-xs">{marketCapDisplay} SOL</p>
              </div>
            </div>

            {/* Creator & Created */}
            <div className="mt-2 pt-2 flex items-center justify-between">
              {token.creator ? (
                <div className="flex items-center gap-1">
                  <span className="text-white/40 text-xs">Creator</span>
                  <VerifiedBadge wallet={token.creator!} />
                  <button
                    onClick={(e) => {
                      e.preventDefault()
                      e.stopPropagation()
                      navigator.clipboard.writeText(token.creator!)
                      setCreatorCopied(true)
                      setTimeout(() => setCreatorCopied(false), 1500)
                    }}
                    className="flex items-center gap-1 text-white/40 hover:text-white/70 transition-colors cursor-pointer"
                    title="Copy creator address"
                  >
                    <span className="text-xs font-mono">
                      {shortenAddress(token.creator)}
                    </span>
                    {creatorCopied ? (
                      <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-success"><polyline points="20 6 9 17 4 12" /></svg>
                    ) : (
                      <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2" /><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" /></svg>
                    )}
                  </button>
                </div>
              ) : (
                <span className="text-white/20 text-xs">...</span>
              )}
              {token.last_activity_at && currentSlot && currentSlot > BigInt(0) && (
                <span className="text-white/30 text-xs">
                  Active {formatSlotAge(BigInt(token.last_activity_at), currentSlot)}
                </span>
              )}
            </div>
          </div>
        </div>
      </div>
    </Link>
  )
}
