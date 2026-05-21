import { useState, useEffect } from 'react'

export interface SaidVerification {
  verified: boolean
  trustTier: 'high' | 'medium' | 'low' | null
  name?: string
  loading: boolean
}

const cache = new Map<string, SaidVerification>()

export function useSaidVerification(wallet: string | null): SaidVerification {
  const [state, setState] = useState<SaidVerification>({
    verified: false,
    trustTier: null,
    loading: true,
  })

  useEffect(() => {
    if (!wallet) {
      setState({ verified: false, trustTier: null, loading: false })
      return
    }

    // Check cache first
    const cached = cache.get(wallet)
    if (cached) {
      setState(cached)
      return
    }

    // Fetch from SAID API
    const controller = new AbortController()

    fetch(`/api/v1/said/verify/${wallet}`, {
      signal: controller.signal,
    })
      .then((res) => res.json())
      .then((data) => {
        const result: SaidVerification = {
          verified: data.verified ?? false,
          trustTier: data.reputation?.trustTier ?? null,
          name: data.identity?.name,
          loading: false,
        }
        cache.set(wallet, result)
        setState(result)
      })
      .catch((err) => {
        if (err.name !== 'AbortError') {
          const result: SaidVerification = {
            verified: false,
            trustTier: null,
            loading: false,
          }
          cache.set(wallet, result)
          setState(result)
        }
      })

    return () => controller.abort()
  }, [wallet])

  return state
}

// Batch verification for multiple wallets (messages)
export function useSaidVerificationBatch(wallets: string[]): Map<string, SaidVerification> {
  const [results, setResults] = useState<Map<string, SaidVerification>>(new Map())

  useEffect(() => {
    if (wallets.length === 0) return

    const uniqueWallets = [...new Set(wallets)]
    const newResults = new Map<string, SaidVerification>()

    // Check cache first, mark uncached for fetch
    const toFetch: string[] = []
    for (const wallet of uniqueWallets) {
      const cached = cache.get(wallet)
      if (cached) {
        newResults.set(wallet, cached)
      } else {
        newResults.set(wallet, { verified: false, trustTier: null, loading: true })
        toFetch.push(wallet)
      }
    }

    setResults(new Map(newResults))

    if (toFetch.length === 0) return

    // Fetch uncached wallets (in parallel, limited concurrency)
    const controller = new AbortController()

    Promise.all(
      toFetch.map((wallet) =>
        fetch(`/api/v1/said/verify/${wallet}`, {
          signal: controller.signal,
        })
          .then((res) => res.json())
          .then((data) => ({
            wallet,
            result: {
              verified: data.verified ?? false,
              trustTier: data.reputation?.trustTier ?? null,
              name: data.identity?.name,
              loading: false,
            } as SaidVerification,
          }))
          .catch(() => ({
            wallet,
            result: {
              verified: false,
              trustTier: null,
              loading: false,
            } as SaidVerification,
          })),
      ),
    ).then((fetchedResults) => {
      const updated = new Map(newResults)
      for (const { wallet, result } of fetchedResults) {
        cache.set(wallet, result)
        updated.set(wallet, result)
      }
      setResults(updated)
    })

    return () => controller.abort()
  }, [wallets.join(',')])

  return results
}
