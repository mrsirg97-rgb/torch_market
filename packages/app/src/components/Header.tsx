'use client'

import { useState, useEffect, useRef } from 'react'
import dynamic from 'next/dynamic'
import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { useTheme } from '@/lib/ThemeContext'
import { useNetwork } from '@/lib/NetworkContext'
import { NetworkDropdown } from '@/components/NetworkDropdown'

const WalletMultiButton = dynamic(
  () => import('@solana/wallet-adapter-react-ui').then((mod) => mod.WalletMultiButton),
  { ssr: false },
)


interface HeaderProps {
  onHowItWorksClick?: () => void
  onTreasuryClick?: () => void
  // Legacy props from the old slide-over layout — accepted to keep callers
  // compiling during the redesign; will be removed once every page lands.
  onUserClick?: () => void
  panelOpen?: boolean
  userPanelOpen?: boolean
  hideUserButton?: boolean
}

/**
 * Floating, transparent nav. Sits over content; the action buttons are
 * pointer-targets, everything between them lets clicks pass through.
 * On mobile, the nav links collapse into a hamburger dropdown.
 */
export function Header({ onHowItWorksClick, onTreasuryClick }: HeaderProps) {
  const { theme, toggleTheme } = useTheme()
  const { networkId, setNetworkId } = useNetwork()
  const pathname = usePathname()
  const [menuOpen, setMenuOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  // Close hamburger on outside click
  useEffect(() => {
    if (!menuOpen) return
    const onClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false)
      }
    }
    document.addEventListener('mousedown', onClick)
    return () => document.removeEventListener('mousedown', onClick)
  }, [menuOpen])

  const linkStyle = (active: boolean) => ({
    color: active ? 'var(--accent)' : 'var(--foreground)',
  })

  const isHome = pathname === '/'
  const isMarkets = pathname?.startsWith('/markets')
  const isProfile = pathname?.startsWith('/profile')

  return (
    <header
      className="sticky top-0 z-50 pointer-events-none"
      style={{
        background: 'color-mix(in srgb, var(--background) 35%, transparent)',
        backdropFilter: 'blur(8px)',
        WebkitBackdropFilter: 'blur(8px)',
      }}
    >
      <div className="w-full px-4 sm:px-6 lg:px-8 py-4 flex items-center justify-between">
        {/* Wordmark */}
        <Link
          href="/"
          className="pointer-events-auto inline-flex items-baseline gap-0"
          style={{ color: 'var(--foreground)' }}
        >
          <span className="text-lg sm:text-xl font-bold">torch</span>
          <span
            className="hidden sm:inline text-lg sm:text-xl"
            style={{ color: 'var(--muted)', fontWeight: 400 }}
          >
            .market
          </span>
        </Link>

        {/* Right cluster — consistent gap across all items */}
        <nav className="pointer-events-auto flex items-center gap-4">
          {/* Desktop nav links */}
          <Link
            href="/"
            className="hidden sm:inline-flex text-sm"
            style={linkStyle(!!isHome)}
          >
            home
          </Link>
          <Link
            href="/markets"
            className="hidden sm:inline-flex text-sm"
            style={linkStyle(!!isMarkets)}
          >
            markets
          </Link>
          <Link
            href="/profile"
            className="hidden sm:inline-flex text-sm"
            style={linkStyle(!!isProfile)}
          >
            stats
          </Link>
          {onTreasuryClick && (
            <button
              onClick={onTreasuryClick}
              className="hidden sm:inline-flex text-sm cursor-pointer"
              style={{ color: 'var(--muted)' }}
              title="Protocol treasury"
            >
              treasury
            </button>
          )}
          {onHowItWorksClick && (
            <button
              onClick={onHowItWorksClick}
              className="hidden sm:inline-flex text-sm cursor-pointer"
              style={{ color: 'var(--muted)' }}
              title="How it works"
            >
              how
            </button>
          )}

          {/* Mobile hamburger — same items collapsed */}
          <div className="sm:hidden relative" ref={menuRef}>
            <button
              onClick={() => setMenuOpen((v) => !v)}
              className="w-7 h-7 flex items-center justify-center cursor-pointer"
              style={{ color: 'var(--foreground)' }}
              aria-label="Menu"
              aria-expanded={menuOpen}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <line x1="3" y1="6" x2="21" y2="6" />
                <line x1="3" y1="12" x2="21" y2="12" />
                <line x1="3" y1="18" x2="21" y2="18" />
              </svg>
            </button>
            {menuOpen && (
              <div
                className="absolute right-0 mt-2 min-w-[160px] rounded-xl py-2 z-[60]"
                style={{
                  background: 'color-mix(in srgb, var(--background) 96%, transparent)',
                  boxShadow: '0 12px 32px rgba(0,0,0,0.25)',
                  backdropFilter: 'blur(12px)',
                }}
              >
                <Link
                  href="/"
                  onClick={() => setMenuOpen(false)}
                  className="block px-4 py-2 text-sm"
                  style={linkStyle(!!isHome)}
                >
                  home
                </Link>
                <Link
                  href="/markets"
                  onClick={() => setMenuOpen(false)}
                  className="block px-4 py-2 text-sm"
                  style={linkStyle(!!isMarkets)}
                >
                  markets
                </Link>
                <Link
                  href="/profile"
                  onClick={() => setMenuOpen(false)}
                  className="block px-4 py-2 text-sm"
                  style={linkStyle(!!isProfile)}
                >
                  stats
                </Link>
                {onHowItWorksClick && (
                  <button
                    onClick={() => {
                      setMenuOpen(false)
                      onHowItWorksClick()
                    }}
                    className="block w-full text-left px-4 py-2 text-sm cursor-pointer"
                    style={{ color: 'var(--muted)' }}
                  >
                    how
                  </button>
                )}
                {onTreasuryClick && (
                  <button
                    onClick={() => {
                      setMenuOpen(false)
                      onTreasuryClick()
                    }}
                    className="block w-full text-left px-4 py-2 text-sm cursor-pointer"
                    style={{ color: 'var(--muted)' }}
                  >
                    treasury
                  </button>
                )}
              </div>
            )}
          </div>

          {/* Always-visible: network, theme, wallet */}
          <NetworkDropdown />

          <button
            onClick={toggleTheme}
            className="inline-flex items-center text-sm cursor-pointer leading-none"
            style={{ color: 'var(--muted)' }}
            title={theme === 'dark' ? 'Light mode' : 'Dark mode'}
          >
            {theme === 'dark' ? '☀' : '☾'}
          </button>

          <WalletMultiButton />
        </nav>
      </div>
    </header>
  )
}
