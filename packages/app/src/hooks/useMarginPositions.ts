'use client'

import { useEffect, useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { getPosition } from 'torchsdk'
import type { PositionInfo } from 'torchsdk'

const isDev = process.env.NODE_ENV === 'development'

export interface MarginPositionEntry {
  mint: string
  loan: PositionInfo | null
  short: PositionInfo | null
}

/**
 * Fetches loan and short positions for the connected wallet, scoped to the
 * given mint list. Empty positions (no borrow / no short collateral) are
 * filtered out. Returns [] when wallet disconnected.
 */
export function useMarginPositions(heldMints: string[]): {
  positions: MarginPositionEntry[]
  loading: boolean
} {
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const [positions, setPositions] = useState<MarginPositionEntry[]>([])
  const [loading, setLoading] = useState(false)
  const mintsKey = heldMints.join(',')

  useEffect(() => {
    let cancelled = false

    const run = async () => {
      if (!publicKey || heldMints.length === 0) {
        setPositions([])
        return
      }

      setLoading(true)
      try {
        const wallet = publicKey.toString()
        const results = await Promise.all(
          heldMints.map(async (mint) => {
            const [loan, short] = await Promise.all([
              getPosition(connection, mint, wallet, 'long').catch(() => null),
              getPosition(connection, mint, wallet, 'short').catch(() => null),
            ])
            // [V21] unified PositionInfo: long debt is SOL, short collateral is SOL.
            const hasLoan = loan && loan.debt_amount > 0
            const hasShort = short && short.collateral_amount > 0
            if (!hasLoan && !hasShort) return null
            return {
              mint,
              loan: hasLoan ? loan : null,
              short: hasShort ? short : null,
            } as MarginPositionEntry
          }),
        )
        if (cancelled) return
        setPositions(results.filter((r): r is MarginPositionEntry => r !== null))
      } catch (err) {
        if (isDev) console.error('useMarginPositions: failed to fetch', err)
        if (!cancelled) setPositions([])
      } finally {
        if (!cancelled) setLoading(false)
      }
    }

    run()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [publicKey, connection, mintsKey])

  return { positions, loading }
}
