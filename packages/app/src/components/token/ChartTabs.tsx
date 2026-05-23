'use client'

import { useState, useEffect, useMemo } from 'react'
import { useConnection } from '@solana/wallet-adapter-react'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync } from '@solana/spl-token'
import {
  getBondingCurvePda,
  getTokenTreasuryPda,
  getTreasuryLockPda,
  getDeepPoolAccounts,
} from 'torchsdk'
import { PriceChart } from '@/components'
import type { PricePoint } from '@/lib/trades'
import {
  formatTokens,
  shortenAddress,
  TOKEN_DECIMALS,
  TOKEN_2022_PROGRAM_ID,
} from '@/lib/constants'

interface ChartTabsProps {
  mint: PublicKey
  priceInSol: number
  solRaised: number
  solPriceUsd: number | null
  priceHistory: PricePoint[]
}

type TabId = 'chart' | 'leaderboard' | 'bubbles'

interface TokenHolder {
  address: string
  balance: bigint
  percentage: number
}


export function ChartTabs({ mint, priceInSol, solRaised, solPriceUsd, priceHistory }: ChartTabsProps) {
  const { connection } = useConnection()
  const [activeTab, setActiveTab] = useState<TabId>('chart')
  const [holders, setHolders] = useState<TokenHolder[]>([])
  const [holdersLoading, setHoldersLoading] = useState(false)
  const [holdersError, setHoldersError] = useState<string | null>(null)

  const tabs: { id: TabId; label: string; icon: React.ReactNode }[] = [
    {
      id: 'chart',
      label: 'Chart',
      icon: (
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
        >
          <line x1="12" y1="20" x2="12" y2="10" />
          <line x1="18" y1="20" x2="18" y2="4" />
          <line x1="6" y1="20" x2="6" y2="16" />
        </svg>
      ),
    },
    {
      id: 'leaderboard',
      label: 'Holders',
      icon: (
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
        >
          <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
          <circle cx="9" cy="7" r="4" />
          <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
          <path d="M16 3.13a4 4 0 0 1 0 7.75" />
        </svg>
      ),
    },
    {
      id: 'bubbles',
      label: 'Bubbles',
      icon: (
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
        >
          <circle cx="12" cy="12" r="10" />
          <circle cx="12" cy="12" r="6" />
          <circle cx="12" cy="12" r="2" />
        </svg>
      ),
    },
  ]

  // Get pool/vault addresses to filter out
  const excludedAddresses = useMemo(() => {
    const excluded = new Set<string>()

    // Bonding curve token vault
    const [bondingCurvePda] = getBondingCurvePda(mint)
    const bondingCurveVault = getAssociatedTokenAddressSync(
      mint,
      bondingCurvePda,
      true,
      TOKEN_2022_PROGRAM_ID,
    )
    excluded.add(bondingCurveVault.toString())

    // Treasury token account
    const [treasuryPda] = getTokenTreasuryPda(mint)
    const treasuryVault = getAssociatedTokenAddressSync(
      mint,
      treasuryPda,
      true,
      TOKEN_2022_PROGRAM_ID,
    )
    excluded.add(treasuryVault.toString())

    // Treasury lock PDA token account (30% locked supply)
    try {
      const [treasuryLockPda] = getTreasuryLockPda(mint)
      const treasuryLockVault = getAssociatedTokenAddressSync(
        mint,
        treasuryLockPda,
        true,
        TOKEN_2022_PROGRAM_ID,
      )
      excluded.add(treasuryLockVault.toString())
    } catch {
      // Ignore if treasury lock can't be derived
    }

    // DeepPool token vault (if migrated)
    try {
      const { tokenVault } = getDeepPoolAccounts(mint)
      excluded.add(tokenVault.toString())
    } catch {
      // Ignore if DeepPool accounts can't be derived
    }

    return excluded
  }, [mint])

  // Fetch token holders when leaderboard or bubbles tab is active
  useEffect(() => {
    if (activeTab === 'chart' || holders.length > 0) return

    async function fetchHolders() {
      setHoldersLoading(true)
      setHoldersError(null)

      try {
        // Use getTokenLargestAccounts for Token2022
        const response = await connection.getTokenLargestAccounts(mint, 'confirmed')

        const totalSupply = BigInt(1_000_000_000) * BigInt(10 ** TOKEN_DECIMALS) // 1B tokens

        const filtered = response.value
          .filter((account) => account.uiAmount && account.uiAmount > 0)
          .filter((account) => !excludedAddresses.has(account.address.toString()))
          .slice(0, 20)

        // Resolve token account addresses to wallet owner addresses
        const accountInfos = await connection.getMultipleParsedAccounts(
          filtered.map((a) => a.address),
        )

        const holderData: TokenHolder[] = filtered.map((account, i) => {
          const parsed = accountInfos.value[i]?.data
          const owner = parsed && 'parsed' in parsed ? parsed.parsed?.info?.owner : null
          return {
            address: owner || account.address.toString(),
            balance: BigInt(account.amount),
            percentage: (Number(account.amount) / Number(totalSupply)) * 100,
          }
        })

        setHolders(holderData)
      } catch (err) {
        console.error('Error fetching holders:', err)
        setHoldersError('Failed to load holders')
      } finally {
        setHoldersLoading(false)
      }
    }

    fetchHolders()
  }, [activeTab, connection, mint, holders.length, excludedAddresses])

  return (
    <div className="h-full flex flex-col">
      {/* Tabs */}
      <div className="flex gap-1 mb-3">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer ${
              activeTab === tab.id
                ? 'bg-accent/20 text-accent'
                : 'bg-white/5 text-white/50 hover:bg-white/10 hover:text-white/70'
            }`}
          >
            {tab.icon}
            {tab.label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 min-h-0">
        {activeTab === 'chart' && <PriceChart mint={mint.toBase58()} priceInSol={priceInSol} solRaised={solRaised} solPriceUsd={solPriceUsd} priceHistory={priceHistory} />}
        {activeTab === 'leaderboard' && (
          <LeaderboardView holders={holders} loading={holdersLoading} error={holdersError} />
        )}
        {activeTab === 'bubbles' && (
          <BubbleMapView holders={holders} loading={holdersLoading} error={holdersError} />
        )}
      </div>
    </div>
  )
}

// Leaderboard component
function LeaderboardView({
  holders,
  loading,
  error,
}: {
  holders: TokenHolder[]
  loading: boolean
  error: string | null
}) {
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">Loading holders...</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-danger">{error}</div>
      </div>
    )
  }

  if (holders.length === 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">No holders found</div>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto">
      <table className="w-full text-sm">
        <thead className="sticky top-0 bg-[var(--background)]">
          <tr className="text-white/50 text-xs">
            <th className="text-left py-2 px-2">#</th>
            <th className="text-left py-2 px-2">Address</th>
            <th className="text-right py-2 px-2">Balance</th>
            <th className="text-right py-2 px-2">%</th>
          </tr>
        </thead>
        <tbody>
          {holders.map((holder, index) => (
            <tr
              key={holder.address}
              className="border-t border-white/5 hover:bg-white/5 transition-colors"
            >
              <td className="py-2 px-2 text-white/30">{index + 1}</td>
              <td className="py-2 px-2">
                <a
                  href={`https://solscan.io/account/${holder.address}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent hover:underline font-mono"
                >
                  {shortenAddress(holder.address)}
                </a>
              </td>
              <td className="py-2 px-2 text-right font-mono text-white/70">
                {formatTokens(holder.balance)}
              </td>
              <td className="py-2 px-2 text-right">
                <span
                  className={`font-mono ${
                    holder.percentage >= 2
                      ? 'text-danger'
                      : holder.percentage >= 1
                        ? 'text-yellow-400'
                        : 'text-white/50'
                  }`}
                >
                  {holder.percentage.toFixed(2)}%
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// Bubble Map component
function BubbleMapView({
  holders,
  loading,
  error,
}: {
  holders: TokenHolder[]
  loading: boolean
  error: string | null
}) {
  // Calculate bubble sizes and positions
  const bubbles = useMemo(() => {
    if (holders.length === 0) return []

    const maxPercentage = Math.max(...holders.map((h) => h.percentage))
    const minSize = 30
    const maxSize = 120

    return holders.map((holder, index) => {
      // Size based on percentage
      const size = minSize + (holder.percentage / maxPercentage) * (maxSize - minSize)

      // Position in a spiral-like pattern
      const angle = index * 0.8
      const radius = 100 + index * 15
      const x = 50 + Math.cos(angle) * (radius / 4)
      const y = 50 + Math.sin(angle) * (radius / 4)

      return {
        ...holder,
        size,
        x: Math.max(10, Math.min(90, x)),
        y: Math.max(10, Math.min(90, y)),
        color: getHolderColor(holder.percentage),
      }
    })
  }, [holders])

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">Loading distribution...</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-danger">{error}</div>
      </div>
    )
  }

  if (holders.length === 0) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-white/50">No holders found</div>
      </div>
    )
  }

  return (
    <div className="h-full relative overflow-hidden">
      {/* Legend */}
      <div className="absolute top-2 right-2 bg-black/50 rounded-lg p-2 text-xs z-10">
        <div className="flex items-center gap-2 mb-1">
          <div className="w-3 h-3 rounded-full bg-danger" />
          <span className="text-white/70">≥2% (whale)</span>
        </div>
        <div className="flex items-center gap-2 mb-1">
          <div className="w-3 h-3 rounded-full bg-yellow-400" />
          <span className="text-white/70">≥1%</span>
        </div>
        <div className="flex items-center gap-2">
          <div className="w-3 h-3 rounded-full bg-accent" />
          <span className="text-white/70">&lt;1%</span>
        </div>
      </div>

      {/* Bubbles */}
      <svg viewBox="0 0 100 100" className="w-full h-full">
        {bubbles.map((bubble) => (
          <g key={bubble.address}>
            <circle
              cx={bubble.x}
              cy={bubble.y}
              r={bubble.size / 10}
              fill={bubble.color}
              fillOpacity={0.6}
              stroke={bubble.color}
              strokeWidth={0.3}
              className="transition-all duration-300 hover:fill-opacity-80 cursor-pointer"
            >
              <title>
                {shortenAddress(bubble.address)}
                {'\n'}
                {formatTokens(bubble.balance)} ({bubble.percentage.toFixed(2)}%)
              </title>
            </circle>
            {bubble.size > 60 && (
              <text
                x={bubble.x}
                y={bubble.y}
                textAnchor="middle"
                dominantBaseline="middle"
                fill="white"
                fontSize={1.5}
                fontFamily="monospace"
              >
                {bubble.percentage.toFixed(1)}%
              </text>
            )}
          </g>
        ))}
      </svg>

      {/* Stats */}
      <div className="absolute bottom-2 left-2 bg-black/50 rounded-lg p-2 text-xs">
        <p className="text-white/70">
          Top {holders.length} holders:{' '}
          <span className="text-white font-mono">
            {holders.reduce((sum, h) => sum + h.percentage, 0).toFixed(1)}%
          </span>
        </p>
      </div>
    </div>
  )
}

// Helper function to get color based on holding percentage
function getHolderColor(percentage: number): string {
  if (percentage >= 2) return '#ef4444' // danger/red for whales
  if (percentage >= 1) return '#facc15' // yellow for significant holders
  return '#6ee7b7' // accent/green for smaller holders
}
