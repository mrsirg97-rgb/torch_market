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
} from 'lightweight-charts'
import type { PricePoint } from '@/lib/trades'

interface PriceChartProps {
  priceInSol: number
  solRaised: number
  solPriceUsd: number | null
  priceHistory: PricePoint[]
}

type TimeInterval = '1m' | '5m' | '15m' | '1H' | '4H'

const TIME_INTERVALS: { label: string; value: TimeInterval }[] = [
  { label: '1m', value: '1m' },
  { label: '5m', value: '5m' },
  { label: '15m', value: '15m' },
  { label: '1H', value: '1H' },
  { label: '4H', value: '4H' },
]

// Get number of minutes for each interval
function getIntervalMinutes(interval: TimeInterval): number {
  switch (interval) {
    case '1m':
      return 1
    case '5m':
      return 5
    case '15m':
      return 15
    case '1H':
      return 60
    case '4H':
      return 240
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
  intervalMinutes: number,
  priceMultiplier: number,
): { candles: CandlestickData<Time>[]; volumes: HistogramData<Time>[] } {
  if (points.length === 0) return { candles: [], volumes: [] }

  const intervalSeconds = intervalMinutes * 60

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

export function PriceChart({ priceInSol, solRaised, solPriceUsd, priceHistory }: PriceChartProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null)
  const chartRef = useRef<IChartApi | null>(null)
  const candlestickSeriesRef = useRef<ISeriesApi<'Candlestick'> | null>(null)
  const volumeSeriesRef = useRef<ISeriesApi<'Histogram'> | null>(null)
  const priceLineRef = useRef<ReturnType<ISeriesApi<'Candlestick'>['createPriceLine']> | null>(null)
  const [selectedInterval, setSelectedInterval] = useState<TimeInterval>('5m')
  const [hoveredCandle, setHoveredCandle] = useState<{
    open: number; high: number; low: number; close: number
  } | null>(null)

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

  // Build OHLC data from real trades (empty until priceHistory loads)
  const { candleData, volumeData, priceChange, currentPrice } = useMemo(() => {
    const priceMultiplier = solPriceUsd ?? 1
    const intervalMinutes = getIntervalMinutes(selectedInterval)

    const { candles, volumes } = buildCandlesFromHistory(
      priceHistory,
      intervalMinutes,
      priceMultiplier,
    )

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
  }, [solPriceUsd, priceHistory, selectedInterval])

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
        secondsVisible: false,
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
    if (candlestickSeriesRef.current && volumeSeriesRef.current) {
      candlestickSeriesRef.current.setData(candleData)
      volumeSeriesRef.current.setData(volumeData)

      // Remove existing price line
      if (priceLineRef.current) {
        candlestickSeriesRef.current.removePriceLine(priceLineRef.current)
      }

      // Add horizontal line at current price
      if (currentPrice) {
        priceLineRef.current = candlestickSeriesRef.current.createPriceLine({
          price: currentPrice.price,
          color: currentPrice.isUp ? '#22c55e' : '#ef4444',
          lineWidth: 1,
          lineStyle: 2, // Dashed
          axisLabelVisible: false,
          title: '',
        })
      }

      chartRef.current?.timeScale().fitContent()
    }
  }, [candleData, volumeData, currentPrice])

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
