'use client'

import { useState, useMemo, useEffect } from 'react'
import { Header, TokenRow, TreasuryModal, HowItWorksModal } from '@/components'
import {
  CreateTokenPanel,
  FilterDropdown,
  TopProjectsCarousel,
  TrendingCarousel,
} from '@/components/home'
import { useTokens, filterAndSortTokens } from '@/hooks/useTokens'
import { TokenFilter, matchesFilter } from '@/types/token'

export default function MarketsPage() {
  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [showTreasuryModal, setShowTreasuryModal] = useState(false)
  const [showCreatePanel, setShowCreatePanel] = useState(false)
  const [filter, setFilter] = useState<TokenFilter>('bonding')
  const [search, setSearch] = useState('')

  const { tokens, loading, topProjects, trendingTokens, currentSlot } = useTokens()

  const filterCounts = useMemo(
    () => ({
      bonding: tokens.filter((t) => matchesFilter(t, 'bonding')).length,
      complete: tokens.filter((t) => matchesFilter(t, 'complete')).length,
      reclaimed: tokens.filter((t) => matchesFilter(t, 'reclaimed')).length,
      legacy: tokens.filter((t) => matchesFilter(t, 'legacy')).length,
    }),
    [tokens],
  )

  const filteredMarkets = useMemo(
    () => filterAndSortTokens(tokens, filter, search),
    [tokens, filter, search],
  )

  // Incremental rendering — show PAGE_SIZE at a time, load more on window scroll
  const PAGE_SIZE = 50
  const filterKey = `${filter}:${search}`
  const [visibleState, setVisibleState] = useState({ count: PAGE_SIZE, key: filterKey })
  const visibleCount = visibleState.key === filterKey ? visibleState.count : PAGE_SIZE

  const visibleMarkets = useMemo(
    () => filteredMarkets.slice(0, visibleCount),
    [filteredMarkets, visibleCount],
  )

  useEffect(() => {
    const onScroll = () => {
      const scrollBottom = window.innerHeight + window.scrollY
      if (scrollBottom >= document.documentElement.scrollHeight - 600) {
        setVisibleState((prev) => ({
          ...prev,
          count: Math.min(prev.count + PAGE_SIZE, filteredMarkets.length),
        }))
      }
    }
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => window.removeEventListener('scroll', onScroll)
  }, [filteredMarkets.length])

  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header
        onHowItWorksClick={() => setShowHowItWorks(true)}
        onTreasuryClick={() => setShowTreasuryModal(true)}
      />

      <main className="px-4 sm:px-6 lg:px-8 pb-16">
        <div className="max-w-5xl mx-auto">
          {/* Hero */}
          <div className="mb-8 pt-2">
            <h3
              className="text-lg sm:text-3xl font-bold tracking-tight"
              style={{ color: 'var(--foreground)' }}
            >
              markets
            </h3>
            <p className="text-sm mt-1" style={{ color: 'var(--muted)' }}>
              browse, trade, lend, short. all on chain.
            </p>
          </div>

          {/* Carousels */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-8">
            <TopProjectsCarousel tokens={topProjects} />
            <TrendingCarousel tokens={trendingTokens} />
          </div>

          {/* Controls */}
          <div className="flex items-center gap-2 sm:gap-3 mb-4">
            <FilterDropdown value={filter} onChange={setFilter} counts={filterCounts} size="sm" />
            <input
              type="search"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="search"
              className="flex-1 min-w-0 text-sm"
            />
            <button
              onClick={() => setShowCreatePanel(true)}
              className="btn btn-accent text-sm px-3 sm:px-4 py-2 whitespace-nowrap cursor-pointer shrink-0"
              title="Launch a new market"
              aria-label="Launch a new market"
            >
              <span className="sm:hidden">+</span>
              <span className="hidden sm:inline">+ launch</span>
            </button>
          </div>

          {/* List */}
          {loading ? (
            <p className="text-sm py-12 text-center" style={{ color: 'var(--muted)' }}>
              loading markets…
            </p>
          ) : filteredMarkets.length === 0 ? (
            <div className="text-center py-16">
              <p className="text-sm mb-4" style={{ color: 'var(--muted)' }}>
                {tokens.length === 0
                  ? 'no markets launched yet'
                  : search.trim()
                    ? 'no markets match your search'
                    : 'no markets match filter'}
              </p>
              {tokens.length === 0 && (
                <button onClick={() => setShowCreatePanel(true)} className="btn btn-accent">
                  launch the first market
                </button>
              )}
            </div>
          ) : (
            <div className="flex flex-col gap-2">
              {visibleMarkets.map((market) => (
                <TokenRow key={market.mint} token={market} currentSlot={currentSlot} />
              ))}
              {visibleCount < filteredMarkets.length && (
                <p className="text-center text-xs py-4" style={{ color: 'var(--muted)' }}>
                  {visibleCount} of {filteredMarkets.length}
                </p>
              )}
            </div>
          )}
        </div>
      </main>

      <CreateTokenPanel isOpen={showCreatePanel} onClose={() => setShowCreatePanel(false)} />
      <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
      <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
    </div>
  )
}
