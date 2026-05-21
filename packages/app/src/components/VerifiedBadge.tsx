'use client'

import { useSaidVerification } from '@/hooks/useSaidVerification'

interface VerifiedBadgeProps {
  wallet: string
  showLabel?: boolean
  size?: 'sm' | 'md'
}

export function VerifiedBadge({ wallet, showLabel = false, size = 'sm' }: VerifiedBadgeProps) {
  const { verified, trustTier, loading } = useSaidVerification(wallet)

  if (loading || !verified) return null

  const sizeClasses = size === 'sm' ? 'w-3 h-3' : 'w-4 h-4'
  const textSize = size === 'sm' ? 'text-[10px]' : 'text-xs'

  // Color based on trust tier
  const tierColors = {
    high: 'text-emerald-400',
    medium: 'text-blue-400',
    low: 'text-yellow-400',
  }
  const color = trustTier ? tierColors[trustTier] : 'text-blue-400'

  return (
    <span
      className={`inline-flex items-center gap-0.5 ${color}`}
      title={`SAID Verified${trustTier ? ` (${trustTier} trust)` : ''}`}
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 24 24"
        fill="currentColor"
        className={sizeClasses}
      >
        <path
          fillRule="evenodd"
          d="M8.603 3.799A4.49 4.49 0 0112 2.25c1.357 0 2.573.6 3.397 1.549a4.49 4.49 0 013.498 1.307 4.491 4.491 0 011.307 3.497A4.49 4.49 0 0121.75 12a4.49 4.49 0 01-1.549 3.397 4.491 4.491 0 01-1.307 3.497 4.491 4.491 0 01-3.497 1.307A4.49 4.49 0 0112 21.75a4.49 4.49 0 01-3.397-1.549 4.49 4.49 0 01-3.498-1.306 4.491 4.491 0 01-1.307-3.498A4.49 4.49 0 012.25 12c0-1.357.6-2.573 1.549-3.397a4.49 4.49 0 011.307-3.497 4.49 4.49 0 013.497-1.307zm7.007 6.387a.75.75 0 10-1.22-.872l-3.236 4.53L9.53 12.22a.75.75 0 00-1.06 1.06l2.25 2.25a.75.75 0 001.14-.094l3.75-5.25z"
          clipRule="evenodd"
        />
      </svg>
      {showLabel && <span className={textSize}>Verified</span>}
    </span>
  )
}

// Inline version that uses pre-fetched data (for message lists)
interface VerifiedBadgeInlineProps {
  verified: boolean
  trustTier: 'high' | 'medium' | 'low' | null
  size?: 'sm' | 'md'
}

export function VerifiedBadgeInline({
  verified,
  trustTier,
  size = 'sm',
}: VerifiedBadgeInlineProps) {
  if (!verified) return null

  const sizeClasses = size === 'sm' ? 'w-3 h-3' : 'w-4 h-4'

  const tierColors = {
    high: 'text-emerald-400',
    medium: 'text-blue-400',
    low: 'text-yellow-400',
  }
  const color = trustTier ? tierColors[trustTier] : 'text-blue-400'

  return (
    <span
      className={`inline-flex items-center ${color}`}
      title={`SAID Verified${trustTier ? ` (${trustTier} trust)` : ''}`}
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 24 24"
        fill="currentColor"
        className={sizeClasses}
      >
        <path
          fillRule="evenodd"
          d="M8.603 3.799A4.49 4.49 0 0112 2.25c1.357 0 2.573.6 3.397 1.549a4.49 4.49 0 013.498 1.307 4.491 4.491 0 011.307 3.497A4.49 4.49 0 0121.75 12a4.49 4.49 0 01-1.549 3.397 4.491 4.491 0 01-1.307 3.497 4.491 4.491 0 01-3.497 1.307A4.49 4.49 0 0112 21.75a4.49 4.49 0 01-3.397-1.549 4.49 4.49 0 01-3.498-1.306 4.491 4.491 0 01-1.307-3.498A4.49 4.49 0 012.25 12c0-1.357.6-2.573 1.549-3.397a4.49 4.49 0 011.307-3.497 4.49 4.49 0 013.497-1.307zm7.007 6.387a.75.75 0 10-1.22-.872l-3.236 4.53L9.53 12.22a.75.75 0 00-1.06 1.06l2.25 2.25a.75.75 0 001.14-.094l3.75-5.25z"
          clipRule="evenodd"
        />
      </svg>
    </span>
  )
}
