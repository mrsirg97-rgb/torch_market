/**
 * useCandles — indexer-served OHLCV candles for a mint at a given interval.
 *
 * Server pre-computes the buckets via a window query over `trades ∪ swaps`,
 * so a chart render is O(1) HTTP fetch instead of N RPC walks. Returns
 * `null` when no indexer is configured (caller falls back to client-side
 * synthesis from raw trades).
 */

'use client'

import { useEffect, useState } from 'react'
import { getCandles, type IndexerCandle } from 'torchsdk'
import { useNetwork } from '@/lib/NetworkContext'
import { useTorchFeed } from '@/lib/TorchFeedContext'

export type CandleInterval = '1s' | '15s' | '30s' | '1m' | '5m' | '15m' | '1h' | '4h'

interface UseCandlesResult {
  candles: IndexerCandle[] | null
  loading: boolean
  error: string | null
}

/**
 * @param mint - token mint pubkey
 * @param interval - bucket size (server-supported: 1m / 5m / 15m / 1h / 4h)
 * @param since - optional lower bound (default: 24h ago)
 * @param before - optional upper bound (default: now)
 *
 * Returns `candles: null` if no indexer is configured — that's the signal
 * for the chart to fall back to its client-side synthesis path.
 */
export function useCandles(
  mint: string | undefined,
  interval: CandleInterval,
  since?: Date,
  before?: Date,
): UseCandlesResult {
  const { effectiveIndexerUrl } = useNetwork()
  const [candles, setCandles] = useState<IndexerCandle[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Bumped whenever the WS firehose reports an event relevant to this
  // mint. Drives a re-run of the fetch effect below, so the chart updates
  // live without a page refresh.
  const [refetchTrigger, setRefetchTrigger] = useState(0)

  // Room subscription (prompt-003): the api pre-filters per market, so any
  // frame in this room is chart-relevant. `resync` (incl. reconnects) also
  // refetches — candles fetch is one cheap HTTP + SQL window query.
  useTorchFeed(mint ? { market: mint } : null, (frame) => {
    if (
      frame.kind === 'trade' ||
      frame.kind === 'swap' ||
      frame.kind === 'migration' ||
      frame.kind === 'resync'
    ) {
      setRefetchTrigger((n) => n + 1)
    }
  })

  useEffect(() => {
    if (!effectiveIndexerUrl || !mint) {
      setCandles(null)
      return
    }

    let cancelled = false
    setLoading(true)
    setError(null)

    getCandles({ indexer: effectiveIndexerUrl, mint, interval, since, before })
      .then((rows) => {
        if (!cancelled) {
          setCandles(rows)
          setLoading(false)
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err))
          // Fall through to RPC-synthesized chart on indexer failure.
          setCandles(null)
          setLoading(false)
        }
      })

    return () => {
      cancelled = true
    }
  }, [effectiveIndexerUrl, mint, interval, since, before, refetchTrigger])

  return { candles, loading, error }
}
