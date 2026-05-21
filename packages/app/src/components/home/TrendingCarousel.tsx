'use client'

import { useState, useEffect, useRef, useCallback } from 'react'
import { TokenData } from '@/types/token'
import { TokenCardCompact } from './TokenCardCompact'

interface TrendingCarouselProps {
  tokens: TokenData[]
}

/**
 * Carousel showing trending tokens (150-200 SOL raised)
 */
export function TrendingCarousel({ tokens }: TrendingCarouselProps) {
  const [index, setIndex] = useState(0)
  const touchRef = useRef(0)

  const next = useCallback(() => setIndex((i) => (i + 1) % tokens.length), [tokens.length])
  const prev = useCallback(
    () => setIndex((i) => (i - 1 + tokens.length) % tokens.length),
    [tokens.length],
  )

  // Auto-rotate carousel
  useEffect(() => {
    if (tokens.length <= 1) return
    const interval = setInterval(next, 4000)
    return () => clearInterval(interval)
  }, [tokens.length, next])

  // Clamp index if tokens array shrinks
  const safeIndex = tokens.length > 0 ? index % tokens.length : 0

  return (
    <div
      className="card p-4 relative group"
      onTouchStart={(e) => { touchRef.current = e.touches[0].clientX }}
      onTouchEnd={(e) => {
        const diff = touchRef.current - e.changedTouches[0].clientX
        if (Math.abs(diff) > 40) diff > 0 ? next() : prev()
      }}
    >
      <div className="flex items-center gap-2 mb-3">
        <span
          className="text-sm font-medium lowercase tracking-wide"
          style={{ color: 'var(--muted)' }}
        >
          graduating
        </span>
        {tokens.length > 1 && (
          <span className="text-xs ml-auto" style={{ color: 'var(--muted)' }}>
            {safeIndex + 1}/{tokens.length}
          </span>
        )}
      </div>
      {tokens.length > 0 ? (
        <TokenCardCompact token={tokens[safeIndex]} variant="trending" />
      ) : (
        <p className="text-xs" style={{ color: 'var(--muted)' }}>no markets near migration</p>
      )}
      {/* Desktop chevrons */}
      {tokens.length > 1 && (
        <>
          <button
            onClick={prev}
            className="hidden md:flex absolute left-1 top-1/2 -translate-y-1/2 w-7 h-7 items-center justify-center rounded-full bg-black/60 text-white/50 hover:text-white hover:bg-black/80 opacity-0 group-hover:opacity-100 transition-opacity cursor-pointer"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
          </button>
          <button
            onClick={next}
            className="hidden md:flex absolute right-1 top-1/2 -translate-y-1/2 w-7 h-7 items-center justify-center rounded-full bg-black/60 text-white/50 hover:text-white hover:bg-black/80 opacity-0 group-hover:opacity-100 transition-opacity cursor-pointer"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
          </button>
        </>
      )}
    </div>
  )
}
