'use client'

import { useEffect, useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { getUserStats } from 'torchsdk'
import type { UserStatsInfo } from 'torchsdk'

const isDev = process.env.NODE_ENV === 'development'

/**
 * Fetches the connected wallet's UserStats account from the program.
 * Returns null when the account doesn't exist yet (user has never traded).
 */
export function useUserStats(): { stats: UserStatsInfo | null; loading: boolean } {
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const [stats, setStats] = useState<UserStatsInfo | null>(null)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    let cancelled = false

    const run = async () => {
      if (!publicKey) {
        setStats(null)
        return
      }
      setLoading(true)
      try {
        const result = await getUserStats(connection, publicKey.toString())
        if (cancelled) return
        setStats(result ?? null)
      } catch (err) {
        if (isDev) console.error('useUserStats: fetch failed', err)
        if (!cancelled) setStats(null)
      } finally {
        if (!cancelled) setLoading(false)
      }
    }

    run()
    return () => {
      cancelled = true
    }
  }, [publicKey, connection])

  return { stats, loading }
}
