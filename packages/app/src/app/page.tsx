'use client'

import { useMemo, useState } from 'react'
import dynamic from 'next/dynamic'
import Link from 'next/link'
import { useWallet } from '@solana/wallet-adapter-react'
import { Header, TreasuryModal, HowItWorksModal } from '@/components'
import { TokenCardPortfolio } from '@/components/home/TokenCardPortfolio'
import { RewardsStrip } from '@/components/portfolio/RewardsStrip'
import { MarginPositionsList } from '@/components/portfolio/MarginPositionsList'
import { VaultStrip } from '@/components/portfolio/VaultStrip'
import { UserStatsStrip } from '@/components/portfolio/UserStatsStrip'
import { useTokens } from '@/hooks/useTokens'
import { useHoldings, totalBalance } from '@/hooks/useHoldings'
import { useMarginPositions } from '@/hooks/useMarginPositions'
import { shortenAddress, TOKEN_MULTIPLIER } from '@/lib/constants'

// Wallet connect button — client-only to avoid SSR mismatch
const WalletMultiButton = dynamic(
  () => import('@solana/wallet-adapter-react-ui').then((mod) => mod.WalletMultiButton),
  { ssr: false },
)

function SectionTitle({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="flex items-baseline justify-between mb-3">
      <h2
        className="text-sm font-medium lowercase tracking-wide"
        style={{ color: 'var(--muted)' }}
      >
        {title}
      </h2>
      {hint && (
        <span className="text-xs" style={{ color: 'var(--muted)' }}>
          {hint}
        </span>
      )}
    </div>
  )
}

export default function Home() {
  const { publicKey } = useWallet()
  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [showTreasuryModal, setShowTreasuryModal] = useState(false)

  const { tokens } = useTokens({ enabled: !!publicKey })
  const balances = useHoldings(tokens)

  const heldMarkets = useMemo(
    () => tokens.filter((t) => totalBalance(balances, t.mint) > BigInt(0)),
    [tokens, balances],
  )

  const heldMints = useMemo(() => heldMarkets.map((t) => t.mint), [heldMarkets])
  const { positions: marginPositions, loading: marginLoading } = useMarginPositions(heldMints)

  // Per-position value in SOL + total portfolio value. Cheap: derived from
  // already-fetched balances and the market's spot price_sol.
  const positionValues = useMemo(() => {
    const map = new Map<string, number>()
    for (const t of heldMarkets) {
      const bal = totalBalance(balances, t.mint)
      const tokens = Number(bal) / TOKEN_MULTIPLIER
      map.set(t.mint, tokens * (t.price_sol ?? 0))
    }
    return map
  }, [heldMarkets, balances])

  const portfolioValueSol = useMemo(
    () => Array.from(positionValues.values()).reduce((sum, v) => sum + v, 0),
    [positionValues],
  )

  const createdMarkets = useMemo(() => {
    if (!publicKey) return []
    const wallet = publicKey.toString()
    return tokens.filter((t) => t.creator === wallet)
  }, [tokens, publicKey])

  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header
        onHowItWorksClick={() => setShowHowItWorks(true)}
        onTreasuryClick={() => setShowTreasuryModal(true)}
      />

      <main className="px-4 sm:px-6 lg:px-8 pb-16">
        <div className="max-w-3xl mx-auto">
          {!publicKey ? (
            <ConnectPrompt />
          ) : (
            <>
              {/* Greeting */}
              <div className="mb-8 pt-2">
                <h1
                  className="text-2xl sm:text-3xl font-bold tracking-tight lowercase"
                  style={{ color: 'var(--foreground)' }}
                >
                  hi {shortenAddress(publicKey.toString())}.
                </h1>
                <p className="text-sm mt-1" style={{ color: 'var(--muted)' }}>
                  your markets, in one place.
                </p>
              </div>

              <UserStatsStrip portfolioValueSol={portfolioValueSol} />

              <RewardsStrip />

              {/* Positions */}
              <section className="mb-8">
                <SectionTitle
                  title="positions"
                  hint={heldMarkets.length > 0 ? `${heldMarkets.length}` : undefined}
                />
                {heldMarkets.length === 0 ? (
                  <p className="text-sm" style={{ color: 'var(--muted)' }}>
                    no positions yet.{' '}
                    <Link
                      href="/markets"
                      className="underline underline-offset-4"
                      style={{ color: 'var(--accent)' }}
                    >
                      browse markets
                    </Link>
                  </p>
                ) : (
                  <div className="flex flex-col gap-2">
                    {heldMarkets.map((t) => {
                      const entry = balances.get(t.mint)
                      const value = positionValues.get(t.mint) ?? 0
                      const pct = portfolioValueSol > 0 ? (value / portfolioValueSol) * 100 : 0
                      return (
                        <TokenCardPortfolio
                          key={t.mint}
                          token={t}
                          userBalance={entry ? entry.wallet + entry.vault : undefined}
                          walletBalance={entry?.wallet}
                          vaultBalance={entry?.vault}
                          valueSol={value}
                          portfolioPct={pct}
                        />
                      )
                    })}
                  </div>
                )}
              </section>

              {/* Margin positions — only if any */}
              {(marginPositions.length > 0 || marginLoading) && (
                <section className="mb-8">
                  <SectionTitle title="margin" hint={`${marginPositions.length}`} />
                  <MarginPositionsList
                    positions={marginPositions}
                    tokens={heldMarkets}
                    loading={marginLoading}
                  />
                </section>
              )}

              {/* Created markets */}
              {createdMarkets.length > 0 && (
                <section className="mb-8">
                  <SectionTitle title="launched" hint={`${createdMarkets.length}`} />
                  <div className="flex flex-col gap-2">
                    {createdMarkets.map((t) => (
                      <TokenCardPortfolio key={t.mint} token={t} />
                    ))}
                  </div>
                </section>
              )}

              {/* Vault */}
              <section className="mb-8">
                <SectionTitle title="vault" />
                <VaultStrip />
              </section>
            </>
          )}
        </div>
      </main>

      <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
      <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
    </div>
  )
}

function ConnectPrompt() {
  return (
    <div className="min-h-[70vh] flex flex-col items-center justify-center text-center px-6">
      <h1
        className="text-3xl sm:text-4xl font-bold tracking-tight lowercase mb-2"
        style={{ color: 'var(--foreground)' }}
      >
        every token is a market.
      </h1>
      <p className="text-base mb-8" style={{ color: 'var(--muted)' }}>
        launch, trade, lend, short. all on chain.
      </p>
      <div className="flex flex-col sm:flex-row items-center gap-3">
        <WalletMultiButton />
        <Link
          href="/markets"
          className="text-sm underline underline-offset-4"
          style={{ color: 'var(--muted)' }}
        >
          browse markets first →
        </Link>
      </div>
    </div>
  )
}
