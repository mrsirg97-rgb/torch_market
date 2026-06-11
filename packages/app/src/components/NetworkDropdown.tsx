'use client'

import { useState } from 'react'
import { useNetwork, type NetworkId } from '@/lib/NetworkContext'

// Same visual language as home/FilterDropdown (sm variant) so the header
// reads as one system. Mainnet is greyed out until torch.market exists.
const OPTIONS: { id: NetworkId; label: string; disabled?: boolean; note?: string }[] = [
  { id: 'simnet', label: 'sim' },
  { id: 'devnet', label: 'dev' },
  { id: 'mainnet', label: 'main', disabled: true, note: 'coming soon' },
]

export function NetworkDropdown() {
  const { networkId, setNetworkId } = useNetwork()
  const [isOpen, setIsOpen] = useState(false)

  const current = OPTIONS.find((o) => o.id === networkId)

  const dropdownStyle: React.CSSProperties = {
    background: 'color-mix(in srgb, var(--background) 60%, transparent)',
    backdropFilter: 'blur(8px)',
    WebkitBackdropFilter: 'blur(8px)',
    boxShadow: '0 8px 24px rgba(0,0,0,0.2)',
  }

  return (
    <div className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="btn text-xs px-3 py-2 inline-flex items-center gap-2"
        title="Network"
      >
        <span>{current?.label ?? networkId}</span>
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
          className={`transition-transform ${isOpen ? 'rotate-180' : ''}`}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>

      {isOpen && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setIsOpen(false)} />
          <div
            className="absolute top-full right-0 mt-1 overflow-hidden z-20 min-w-[140px] rounded-xl"
            style={dropdownStyle}
          >
            {OPTIONS.map((option) => (
              <button
                key={option.id}
                disabled={option.disabled}
                title={option.note}
                onClick={() => {
                  if (option.disabled) return
                  setNetworkId(option.id)
                  setIsOpen(false)
                }}
                className={`w-full px-3 py-2 text-xs text-left flex items-center justify-between transition-colors ${
                  option.disabled
                    ? 'text-white/25 cursor-not-allowed'
                    : `hover:bg-white/10 cursor-pointer ${
                        networkId === option.id ? 'bg-white/5 text-white' : 'text-white/70'
                      }`
                }`}
              >
                <span>{option.label}</span>
                {option.disabled && (
                  <span className="text-white/25 text-[10px] italic">{option.note}</span>
                )}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
