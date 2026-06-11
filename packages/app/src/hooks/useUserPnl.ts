/**
 * useUserPnl — realized PnL for the connected wallet, via the indexer.
 *
 * Returns null when no indexer is configured or no wallet is connected.
 * Refetches when the WS firehose reports a trade/swap event for any mint
 * the user has touched.
 */

'use client'

import { useEffect, useState } from 'react'
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
  useTorchFeed(wallet ? 'all' : null, (frame) => {
    if (frame.kind === 'trade' || frame.kind === 'swap' || frame.kind === 'resync') {
      setRefetchTrigger((n) => n + 1)
    }
  })

  useEffect(() => {
    if (!effectiveIndexerUrl || !wallet) {
      setPnl(null)
      return
    }
    let cancelled = false
    setLoading(true)
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
          setPnl(null)
          setLoading(false)
        }
      })
    return () => {
      cancelled = true
    }
  }, [effectiveIndexerUrl, wallet, refetchTrigger])

  return { pnl, loading, error }
}
