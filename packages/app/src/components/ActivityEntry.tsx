'use client'

import Link from 'next/link'
import type { ActivityEntry as ActivityEntryType, ActionType } from '@/hooks/useActivityFeed'

const ACTION_LABELS: Record<ActionType, string> = {
  bought: 'bought',
  sold: 'sold',
  launched: 'launched',
  migrated: 'migrated',
  messaged: 'said in',
  shorted: 'shorted',
  borrowed: 'borrowed against',
}

const ACTION_COLORS: Record<ActionType, string> = {
  bought: 'var(--success)',
  sold: 'var(--danger)',
  launched: 'var(--accent)',
  migrated: 'var(--accent)',
  messaged: 'var(--muted)',
  shorted: 'var(--danger)',
  borrowed: 'var(--muted)',
}

function shortenAddr(addr: string): string {
  return addr.slice(0, 4) + '..' + addr.slice(-3)
}

function timeAgo(ts: number): string {
  const seconds = Math.floor(Date.now() / 1000 - ts)
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}

function fmtSol(sol: number): string {
  if (sol >= 1) return sol.toFixed(2)
  if (sol >= 0.01) return sol.toFixed(3)
  return sol.toFixed(4)
}

export function ActivityEntry({ entry }: { entry: ActivityEntryType }) {
  return (
    <div
      className="border-b"
      style={{ borderColor: 'var(--border-color)', padding: '0.5rem 0.25rem' }}
    >
      <div className="flex items-baseline justify-between gap-2">
        <div className="flex items-baseline gap-1.5 min-w-0 flex-wrap">
          <span className="font-mono text-xs" style={{ color: 'var(--foreground)' }}>
            {shortenAddr(entry.trader)}
          </span>
          <span
            className="text-xs font-medium"
            style={{ color: ACTION_COLORS[entry.action] }}
          >
            {ACTION_LABELS[entry.action]}
          </span>
          <Link
            href={`/markets/${entry.token_mint}`}
            className="text-xs font-medium truncate hover:underline"
            style={{ color: 'var(--foreground)' }}
          >
            {entry.token_name}
          </Link>
          {entry.amount_sol !== null && (
            <span className="font-mono text-xs" style={{ color: 'var(--muted)' }}>
              {fmtSol(entry.amount_sol)} SOL
            </span>
          )}
        </div>
        <span className="font-mono text-xs flex-shrink-0" style={{ color: 'var(--muted)' }}>
          {timeAgo(entry.timestamp)}
        </span>
      </div>
      {entry.memo && (
        <p className="text-xs mt-1 leading-relaxed" style={{ color: 'var(--muted)' }}>
          {entry.memo}
        </p>
      )}
    </div>
  )
}
