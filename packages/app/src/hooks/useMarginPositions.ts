'use client'

import { useEffect, useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { getLoanPosition, getShortPosition } from 'torchsdk'
import type { LoanPositionInfo, ShortPositionInfo } from 'torchsdk'

const isDev = process.env.NODE_ENV === 'development'

export interface MarginPositionEntry {
  mint: string
  loan: LoanPositionInfo | null
  short: ShortPositionInfo | null
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
              getLoanPosition(connection, mint, wallet).catch(() => null),
              getShortPosition(connection, mint, wallet).catch(() => null),
            ])
            const hasLoan = loan && loan.borrowed_amount > 0
            const hasShort = short && short.sol_collateral > 0
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
