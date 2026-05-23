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

  // WS subscription: refetch candles when relevant events land in the
  // indexer's broadcast firehose.
  useEffect(() => {
    if (!effectiveIndexerUrl || !mint) return

    const wsUrl = effectiveIndexerUrl.replace(/^http/, 'ws') + '/events'
    const ws = new WebSocket(wsUrl)

    ws.onmessage = (e: MessageEvent) => {
      try {
        const frame = JSON.parse(e.data as string) as {
          kind?: string
          mint?: string
        }
        // Frames we care about for chart freshness:
        //   - `trade`: bonding-curve buy/sell carrying mint directly
        //   - `migration`: triggers DEX-price extension into the candle set
        //   - `swap`: post-migration DEX trade; pool_id rather than mint, so
        //     we refetch on any swap. Over-refetches at scale, but candle
        //     fetch is cheap (one HTTP + SQL window query).
        const relevant =
          (frame.kind === 'trade' && frame.mint === mint) ||
          (frame.kind === 'migration' && frame.mint === mint) ||
          frame.kind === 'swap'
        if (relevant) {
          setRefetchTrigger((n) => n + 1)
        }
      } catch {
        /* malformed frame — ignore */
      }
    }

    return () => {
      ws.close()
    }
  }, [effectiveIndexerUrl, mint])

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
