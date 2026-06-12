/**
 * useUserPnl — realized PnL for the connected wallet, via the indexer.
 *
 * Returns null when no indexer is configured or no wallet is connected.
 * Refetches when the WS firehose reports a trade/swap event for any mint
 * the user has touched.
 */

'use client'

import { useRef, useEffect, useState } from 'react'
import { useWallet } from '@solana/wallet-adapter-react'
import { getUserPnl, type UserPnlSummary } from 'torchsdk'
import { useNetwork } from '@/lib/NetworkContext'
import { useTorchFeed } from '@/lib/TorchFeedContext'

interface UseUserPnlResult {
  pnl: UserPnlSummary | null
  loading: boolean
  error: string | null
}

export function useUserPnl(): UseUserPnlResult {
  const { publicKey } = useWallet()
  const { effectiveIndexerUrl } = useNetwork()
  const [pnl, setPnl] = useState<UserPnlSummary | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [refetchTrigger, setRefetchTrigger] = useState(0)

  const wallet = publicKey?.toString()

  // WS subscription — refetch on any trade/swap event. We don't filter
  // by user because we don't know which mints the wallet has touched
  // without the PnL data itself; a refetch on any trade is acceptable
  // (cheap HTTP + SQL) and ensures we catch the user's own activity.
  // 'all' room carries trade + swap ticks for every market — enough to know
  // when this wallet's PnL might have moved (refetch recomputes precisely).
  // [prompt-007 family] Debounced: during busy trading the room delivers
  // frames continuously — refetch-per-frame made the stats page re-render
  // storm ("tearing"). One trailing refetch per 2s burst.
  const pnlDebounce = useRef<ReturnType<typeof setTimeout> | null>(null)
  useTorchFeed(wallet ? 'all' : null, (frame) => {
    if (frame.kind === 'trade' || frame.kind === 'swap' || frame.kind === 'resync') {
      if (pnlDebounce.current) clearTimeout(pnlDebounce.current)
      pnlDebounce.current = setTimeout(() => setRefetchTrigger((n) => n + 1), 2000)
    }
  })

  useEffect(() => {
    if (!effectiveIndexerUrl || !wallet) {
      setPnl(null)
      return
    }
    let cancelled = false
    // Stale-while-revalidate: only the INITIAL load shows a loading state.
    // Background refetches keep the current table rendered and swap the data
    // in place when it lands — no skeleton flash, no tearing.
    const isInitial = pnl === null
    if (isInitial) setLoading(true)
    setError(null)
    getUserPnl(effectiveIndexerUrl, wallet)
      .then((summary) => {
        if (!cancelled) {
          setPnl(summary)
          setLoading(false)
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err))
          // Background failure: keep showing stale data rather than blanking
          // the page; only null out if we never had data.
          if (isInitial) setPnl(null)
          setLoading(false)
        }
      })
    return () => {
      cancelled = true
    }
  }, [effectiveIndexerUrl, wallet, refetchTrigger])

  return { pnl, loading, error }
}
