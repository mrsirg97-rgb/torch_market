'use client'

import { useEffect, useRef, useState, useMemo, useCallback } from 'react'
import {
  createChart,
  IChartApi,
  ISeriesApi,
  CandlestickData,
  HistogramData,
  Time,
  MouseEventParams,
  WhitespaceData,
} from 'lightweight-charts'
import type { PricePoint } from '@/lib/trades'
import { useCandles, type CandleInterval } from '@/hooks/useCandles'

interface PriceChartProps {
  mint: string
  priceInSol: number
  solRaised: number
  solPriceUsd: number | null
  priceHistory: PricePoint[]
}

type TimeInterval = '1s' | '15s' | '30s' | '1m' | '5m' | '15m' | '1H' | '4H'

const TIME_INTERVALS: { label: string; value: TimeInterval }[] = [
  { label: '1s', value: '1s' },
  { label: '15s', value: '15s' },
  { label: '30s', value: '30s' },
  { label: '1m', value: '1m' },
  { label: '5m', value: '5m' },
  { label: '15m', value: '15m' },
  { label: '1H', value: '1H' },
  { label: '4H', value: '4H' },
]

// UI interval label → indexer interval value. Sub-minute intervals
// round-trip as-is; we keep the UI labels stylistically uppercase for
// the hour intervals to match conventional charts.
function uiIntervalToIndexer(ui: TimeInterval): CandleInterval {
  switch (ui) {
    case '1s':
      return '1s'
    case '15s':
      return '15s'
    case '30s':
      return '30s'
    case '1m':
      return '1m'
    case '5m':
      return '5m'
    case '15m':
      return '15m'
    case '1H':
      return '1h'
    case '4H':
      return '4h'
  }
}

// Seconds per interval. Used by the client-synthesis fallback path
// (buildCandlesFromHistory); the indexer path computes buckets server-side.
function getIntervalSeconds(interval: TimeInterval): number {
  switch (interval) {
    case '1s':
      return 1
    case '15s':
      return 15
    case '30s':
      return 30
    case '1m':
      return 60
    case '5m':
      return 300
    case '15m':
      return 900
    case '1H':
      return 3600
    case '4H':
      return 14_400
  }
}

/**
 * Build OHLC candles from real price points.
 *
 * Smoothing:
 *  - Each candle's open = previous candle's close (no visual gaps between candles)
 *  - Empty intervals (no trades) are skipped entirely instead of emitting flat candles
 *  - High/low include the open so wicks bridge cleanly to the prior candle
 */
function buildCandlesFromHistory(
  points: PricePoint[],
  intervalSeconds: number,
  priceMultiplier: number,
): { candles: CandlestickData<Time>[]; volumes: HistogramData<Time>[] } {
  if (points.length === 0) return { candles: [], volumes: [] }

  const firstTs = points[0].timestamp
  const lastTs = points[points.length - 1].timestamp
  const firstSlot = Math.floor(firstTs / intervalSeconds) * intervalSeconds
  const lastSlot = Math.floor(lastTs / intervalSeconds) * intervalSeconds

  const candles: CandlestickData<Time>[] = []
  const volumes: HistogramData<Time>[] = []

  let pointIdx = 0
  let prevClose = points[0].price * priceMultiplier

  for (let t = firstSlot; t <= lastSlot; t += intervalSeconds) {
    const slotEnd = t + intervalSeconds

    // Collect points in this candle slot
    const slotPoints: PricePoint[] = []
    while (pointIdx < points.length && points[pointIdx].timestamp < slotEnd) {
      if (points[pointIdx].timestamp >= t) {
        slotPoints.push(points[pointIdx])
      }
      pointIdx++
    }

    // Skip empty intervals — no flat filler candles
    if (slotPoints.length === 0) continue

    const prices = slotPoints.map((p) => p.price * priceMultiplier)
    const close = prices[prices.length - 1]
    // Open connects to previous candle's close for visual continuity
    const open = prevClose
    const high = Math.max(open, ...prices)
    const low = Math.min(open, ...prices)
    const volume = slotPoints.reduce((sum, p) => sum + p.volume, 0)
    prevClose = close

    const isBullish = close >= open

    candles.push({ time: t as Time, open, high, low, close })
    volumes.push({
      time: t as Time,
      value: Math.max(0.001, volume),
      color: isBullish ? 'rgba(34, 197, 94, 0.5)' : 'rgba(239, 68, 68, 0.5)',
    })
  }

  return { candles, volumes }
}

export function PriceChart({ mint, priceInSol, solRaised, solPriceUsd, priceHistory }: PriceChartProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<IChartApi | null>(null)
  const candlestickSeriesRef = useRef<ISeriesApi<'Candlestick'> | null>(null)
  const volumeSeriesRef = useRef<ISeriesApi<'Histogram'> | null>(null)
  const priceLineRef = useRef<ReturnType<ISeriesApi<'Candlestick'>['createPriceLine']> | null>(null)
  const [selectedInterval, setSelectedInterval] = useState<TimeInterval>('5m')
  const [hoveredCandle, setHoveredCandle] = useState<{
    open: number; high: number; low: number; close: number
  } | null>(null)

  // Indexer-served candles (preferred path). Returns `null` when no indexer
  // is configured — the buildCandlesFromHistory fallback then runs.
  const { candles: indexerCandles } = useCandles(mint, uiIntervalToIndexer(selectedInterval))

  // Crosshair handler — updates hoveredCandle as user moves across the chart
  const onCrosshairMove = useCallback((param: MouseEventParams<Time>) => {
    if (!param.time || !candlestickSeriesRef.current) {
      setHoveredCandle(null)
      return
    }
    const data = param.seriesData.get(candlestickSeriesRef.current) as
      | CandlestickData<Time>
      | undefined
    if (data && 'open' in data) {
      setHoveredCandle({ open: data.open, high: data.high, low: data.low, close: data.close })
    } else {
      setHoveredCandle(null)
    }
  }, [])

  // Build OHLC data — prefer indexer-served candles when available, fall
  // back to client-synthesized buckets from raw priceHistory otherwise.
  // The indexer path is O(1) HTTP fetch and computes the OHLCV server-side
  // via a SQL window query over trades ∪ swaps.
  const { candleData, volumeData, priceChange, currentPrice } = useMemo(() => {
    const priceMultiplier = solPriceUsd ?? 1

    let candles: CandlestickData<Time>[]
    let volumes: HistogramData<Time>[]

    if (indexerCandles && indexerCandles.length > 0) {
      // Indexer path. Server-side SQL now computes MARGINAL post-trade
      // price (= reserves_after_sol / reserves_after_tokens) rather than
      // execution price, so each candle reflects the pool's new spot
      // price after the trade settles. That's the right semantic for a
      // public chart: where the curve/pool stands, not the avg price a
      // particular trader experienced.
      //
      // Price scale: the SQL emits raw lamport-per-microtoken. Multiply
      // by 10^-3 to convert to SOL-per-token (matches the SDK's
      // calculatePrice → TOKEN_MULTIPLIER/LAMPORTS_PER_SOL pattern used
      // elsewhere in the app).
      const LAMPORT_TO_SOL_PER_TOKEN = 1e-3
      const finalMultiplier = priceMultiplier * LAMPORT_TO_SOL_PER_TOKEN

      // Force candle N's open = candle N-1's close so wicks/bodies bridge
      // cleanly between buckets. The SQL emits each bucket's open as the
      // first marginal-price-after-trade in that bucket, which doesn't
      // connect to the previous bucket's last price visually. Bridging
      // them makes each candle's BODY represent "price change during this
      // bucket from the previous known state," matching how the original
      // client-synth path renders sparse data.
      const realCandles = indexerCandles.filter(
        (c) => c.open != null && c.high != null && c.low != null && c.close != null,
      )
      let prevClose: number | null = null
      candles = realCandles.map((c) => {
        const t = Math.floor(new Date(c.bucket_start).getTime() / 1000) as Time
        const sqlOpen = (c.open as number) * finalMultiplier
        const sqlHigh = (c.high as number) * finalMultiplier
        const sqlLow = (c.low as number) * finalMultiplier
        const close = (c.close as number) * finalMultiplier
        const open = prevClose ?? sqlOpen
        const high = Math.max(open, sqlHigh, close)
        const low = Math.min(open, sqlLow, close)
        prevClose = close
        return { time: t, open, high, low, close }
      })

      // Volume colors are derived from the BRIDGED open vs close so the
      // histogram color matches the candle body color.
      let prevCloseForVol: number | null = null
      volumes = realCandles.map((c) => {
        const t = Math.floor(new Date(c.bucket_start).getTime() / 1000) as Time
        const close = (c.close as number) * finalMultiplier
        const open = prevCloseForVol ?? (c.open as number) * finalMultiplier
        const isBullish = close >= open
        prevCloseForVol = close
        return {
          time: t,
          value: Math.max(0.001, (c.volume as number) ?? 0),
          color: isBullish ? 'rgba(34, 197, 94, 0.5)' : 'rgba(239, 68, 68, 0.5)',
        }
      })
    } else {
      const intervalSeconds = getIntervalSeconds(selectedInterval)
      const synthesized = buildCandlesFromHistory(
        priceHistory,
        intervalSeconds,
        priceMultiplier,
      )
      candles = synthesized.candles
      volumes = synthesized.volumes
    }

    const firstPrice = candles[0]?.open || 0
    const lastCandle = candles[candles.length - 1]
    const lastPrice = lastCandle?.close || 0
    const change = firstPrice > 0 ? ((lastPrice - firstPrice) / firstPrice) * 100 : 0

    const currentPriceData = lastCandle
      ? { price: lastCandle.close, isUp: lastCandle.close >= lastCandle.open }
      : null

    return {
      candleData: candles,
      volumeData: volumes,
      priceChange: change,
      currentPrice: currentPriceData,
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [solPriceUsd, priceHistory, selectedInterval, indexerCandles])

  // Initialize chart
  useEffect(() => {
    if (!chartContainerRef.current) return

    const chart = createChart(chartContainerRef.current, {
      layout: {
        background: { color: 'transparent' },
        textColor: 'rgba(255, 255, 255, 0.5)',
      },
      grid: {
        vertLines: { color: 'rgba(255, 255, 255, 0.05)' },
        horzLines: { color: 'rgba(255, 255, 255, 0.05)' },
      },
      crosshair: {
        mode: 1,
        vertLine: {
          color: 'rgba(255, 255, 255, 0.3)',
          width: 1,
          style: 2,
          labelBackgroundColor: 'rgba(20, 20, 20, 0.9)',
        },
        horzLine: {
          color: 'rgba(255, 255, 255, 0.3)',
          width: 1,
          style: 2,
          labelBackgroundColor: 'rgba(20, 20, 20, 0.9)',
        },
      },
      rightPriceScale: {
        visible: false,
      },
      timeScale: {
        borderColor: 'rgba(255, 255, 255, 0.1)',
        timeVisible: true,
        secondsVisible: true,
        // Fixed-width candles. lightweight-charts' default behavior is to
        // auto-fit bars across the full chart width — which makes a
        // sparsely-populated chart show comically wide candles. Pinning
        // `barSpacing` keeps every candle the same pixel width regardless
        // of how many candles are loaded. `rightOffset` reserves blank
        // space on the right so the newest candle isn't flush against the
        // edge.
        barSpacing: 6,
        rightOffset: 12,
      },
      handleScale: {
        axisPressedMouseMove: true,
      },
      handleScroll: {
        vertTouchDrag: false,
      },
    })

    // Add candlestick series
    const candlestickSeries = chart.addCandlestickSeries({
      upColor: '#22c55e',
      downColor: '#ef4444',
      borderUpColor: '#22c55e',
      borderDownColor: '#ef4444',
      wickUpColor: '#22c55e',
      wickDownColor: '#ef4444',
      priceFormat: {
        type: 'price',
        precision: 8,
        minMove: 0.00000001,
      },
    })

    // Add volume series
    const volumeSeries = chart.addHistogramSeries({
      priceFormat: {
        type: 'volume',
      },
      priceScaleId: '',
    })

    volumeSeries.priceScale().applyOptions({
      scaleMargins: {
        top: 0.85,
        bottom: 0,
      },
    })

    chartRef.current = chart
    candlestickSeriesRef.current = candlestickSeries
    volumeSeriesRef.current = volumeSeries

    // Track crosshair for OHLC tooltip
    chart.subscribeCrosshairMove(onCrosshairMove)

    // Handle resize
    const handleResize = () => {
      if (chartContainerRef.current && chartRef.current) {
        chartRef.current.applyOptions({
          width: chartContainerRef.current.clientWidth,
          height: chartContainerRef.current.clientHeight,
        })
      }
    }

    const resizeObserver = new ResizeObserver(handleResize)
    resizeObserver.observe(chartContainerRef.current)

    handleResize()

    return () => {
      chart.unsubscribeCrosshairMove(onCrosshairMove)
      resizeObserver.disconnect()
      chart.remove()
      chartRef.current = null
      candlestickSeriesRef.current = null
      volumeSeriesRef.current = null
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Update data when it changes
  useEffect(() => {
    if (!candlestickSeriesRef.current || !volumeSeriesRef.current || !chartRef.current) return

    // Center sparse data by padding both sides with WhitespaceData bars.
    // The chart treats whitespace bars as reserved time slots without
    // rendering anything in them — net effect: real data sits in the
    // middle, surrounded by empty slots that maintain barSpacing.
    //
    // This sidesteps every rightOffset / scrollToPosition / visibleRange
    // quirk we hit: the chart only ever sees N+leftPad+rightPad logical
    // bars, and centers naturally because the data IS centered in the
    // array we hand it.
    const timeScale = chartRef.current.timeScale()
    const chartWidthPx = timeScale.width()
    const BAR_SPACING_PX = 6
    const barsThatFit = Math.floor(chartWidthPx / BAR_SPACING_PX)
    const dataLen = candleData.length

    let paddedCandles: (CandlestickData<Time> | WhitespaceData<Time>)[] = candleData
    let paddedVolumes: (HistogramData<Time> | WhitespaceData<Time>)[] = volumeData

    if (dataLen > 0 && dataLen < barsThatFit) {
      const totalPad = barsThatFit - dataLen
      const leftPad = Math.floor(totalPad / 2)
      const rightPad = totalPad - leftPad
      const intervalSec = getIntervalSeconds(selectedInterval)
      const firstTime = candleData[0].time as number
      const lastTime = candleData[dataLen - 1].time as number

      const leftWhitespace: WhitespaceData<Time>[] = []
      for (let i = leftPad; i > 0; i--) {
        leftWhitespace.push({ time: (firstTime - i * intervalSec) as Time })
      }
      const rightWhitespace: WhitespaceData<Time>[] = []
      for (let i = 1; i <= rightPad; i++) {
        rightWhitespace.push({ time: (lastTime + i * intervalSec) as Time })
      }
      paddedCandles = [...leftWhitespace, ...candleData, ...rightWhitespace]
      paddedVolumes = [...leftWhitespace, ...volumeData, ...rightWhitespace]
    }

    candlestickSeriesRef.current.setData(paddedCandles)
    volumeSeriesRef.current.setData(paddedVolumes)

    if (priceLineRef.current) {
      candlestickSeriesRef.current.removePriceLine(priceLineRef.current)
    }
    if (currentPrice) {
      priceLineRef.current = candlestickSeriesRef.current.createPriceLine({
        price: currentPrice.price,
        color: currentPrice.isUp ? '#22c55e' : '#ef4444',
        lineWidth: 1,
        lineStyle: 2,
        axisLabelVisible: false,
        title: '',
      })
    }
  }, [candleData, volumeData, currentPrice, selectedInterval])

  return (
    <div className="p-4 h-full w-full flex flex-col">
      <div className="flex items-center justify-between mb-4">
        <div className="flex gap-1">
          {TIME_INTERVALS.map(({ label, value }) => (
            <button
              key={value}
              onClick={() => setSelectedInterval(value)}
              className={`px-2 py-1 text-xs rounded-lg font-medium transition-colors cursor-pointer border ${
                selectedInterval === value
                  ? 'border-accent text-white bg-white/5'
                  : 'border-transparent bg-white/10 text-white/70 hover:bg-white/20'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
        <span
          className={`text-sm font-mono ${priceChange >= 0 ? 'text-[#22c55e]' : 'text-[#ef4444]'}`}
        >
          {priceChange >= 0 ? '+' : ''}
          {priceChange.toFixed(2)}%
        </span>
      </div>

      <div className="relative flex-1 min-h-[250px] w-full">
        <div ref={chartContainerRef} className="absolute inset-0" />
        {priceHistory.length === 0 && (
          <div className="absolute inset-0 flex items-center justify-center">
            <span className="text-white/30 text-sm">Loading chart...</span>
          </div>
        )}
        {(() => {
          // Show hovered candle OHLC, or fall back to latest candle
          const candle = hoveredCandle ?? (currentPrice
            ? { open: currentPrice.price, high: currentPrice.price, low: currentPrice.price, close: currentPrice.price }
            : null)
          if (!candle) return null
          const isUp = candle.close >= candle.open
          const fmt = (v: number) => solPriceUsd ? `$${v.toFixed(8)}` : v.toFixed(8)
          return (
            <div className="absolute left-2 top-2 flex items-center gap-3 text-[11px] font-mono pointer-events-none">
              <span className="text-white/40">O <span className={isUp ? 'text-[#22c55e]' : 'text-[#ef4444]'}>{fmt(candle.open)}</span></span>
              <span className="text-white/40">H <span className="text-white/70">{fmt(candle.high)}</span></span>
              <span className="text-white/40">L <span className="text-white/70">{fmt(candle.low)}</span></span>
              <span className="text-white/40">C <span className={isUp ? 'text-[#22c55e]' : 'text-[#ef4444]'}>{fmt(candle.close)}</span></span>
            </div>
          )
        })()}
        {currentPrice && (
          <div
            className={`absolute right-2 top-2 px-2 py-1 text-xs font-mono rounded ${
              (hoveredCandle ? hoveredCandle.close >= hoveredCandle.open : currentPrice.isUp)
                ? 'bg-[#22c55e]' : 'bg-[#ef4444]'
            } text-white transition-all`}
          >
            {solPriceUsd
              ? `$${(hoveredCandle?.close ?? currentPrice.price).toFixed(8)}`
              : `${(hoveredCandle?.close ?? currentPrice.price).toFixed(8)} SOL`}
          </div>
        )}
      </div>
    </div>
  )
}
