'use client'

import { useState } from 'react'
import { TokenFilter, FILTER_LABELS } from '@/types/token'

interface FilterDropdownProps {
  value: TokenFilter
  onChange: (filter: TokenFilter) => void
  counts: Record<TokenFilter, number>
  size?: 'sm' | 'md'
}

const FILTER_OPTIONS: TokenFilter[] = ['bonding', 'complete', 'reclaimed']

/**
 * Dropdown component for filtering tokens by status
 */
export function FilterDropdown({ value, onChange, counts, size = 'md' }: FilterDropdownProps) {
  const [isOpen, setIsOpen] = useState(false)

  const buttonClass =
    size === 'sm'
      ? 'btn text-xs px-3 py-2 inline-flex items-center gap-2'
      : 'btn text-sm px-4 py-2 inline-flex items-center gap-2 min-w-[120px] justify-between'

  const dropdownClass =
    size === 'sm'
      ? 'absolute top-full left-0 mt-1 overflow-hidden z-20 min-w-[140px] rounded-xl'
      : 'absolute top-full left-0 mt-1 overflow-hidden z-20 min-w-[160px] rounded-xl'

  const dropdownStyle: React.CSSProperties = {
    background: 'color-mix(in srgb, var(--background) 60%, transparent)',
    backdropFilter: 'blur(8px)',
    WebkitBackdropFilter: 'blur(8px)',
    boxShadow: '0 8px 24px rgba(0,0,0,0.2)',
  }

  const itemClass =
    size === 'sm'
      ? 'w-full px-3 py-2 text-xs text-left flex items-center justify-between hover:bg-white/10 transition-colors cursor-pointer'
      : 'w-full px-4 py-2 text-sm text-left flex items-center justify-between hover:bg-white/10 transition-colors cursor-pointer'

  const iconSize = size === 'sm' ? '12' : '14'

  return (
    <div className="relative">
      <button onClick={() => setIsOpen(!isOpen)} className={buttonClass}>
        <span>{FILTER_LABELS[value]}</span>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width={iconSize}
          height={iconSize}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={`transition-transform ${isOpen ? 'rotate-180' : ''}`}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>

      {isOpen && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} />
          <div className={dropdownClass} style={dropdownStyle}>
            {FILTER_OPTIONS.map((option) => (
              <button
                key={option}
                onClick={() => {
                  onChange(option)
                  setIsOpen(false)
                }}
                className={`${itemClass} ${
                  value === option ? 'bg-white/5 text-white' : 'text-white/70'
                }`}
              >
                <span>{FILTER_LABELS[option]}</span>
                <span className="text-white/40 text-xs">{counts[option]}</span>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
