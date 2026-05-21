import { NextRequest, NextResponse } from 'next/server'

const RPC_URL = 'https://torch-market-rpc.mrsirg97.workers.dev'

// In-memory cache: mint -> { count, expiry }
const cache = new Map<string, { count: number; expiry: number }>()
const CACHE_TTL_MS = 5 * 60_000 // 5 minutes

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ mint: string }> },
) {
  const { mint } = await params

  // Validate mint (base58, 32-44 chars)
  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(mint)) {
    return NextResponse.json({ error: 'Invalid mint address' }, { status: 400 })
  }

  // Check cache
  const cached = cache.get(mint)
  if (cached && Date.now() < cached.expiry) {
    return NextResponse.json({ holders: cached.count })
  }

  try {
    let total = 0
    let cursor: string | undefined

    // Paginate through all holders
    do {
      const body: Record<string, unknown> = {
        jsonrpc: '2.0',
        id: 'holders',
        method: 'getTokenAccounts',
        params: {
          mint,
          limit: 1000,
          options: { showZeroBalance: false },
          ...(cursor ? { cursor } : {}),
        },
      }

      const res = await fetch(RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })

      const data = await res.json()
      const result = data?.result
      if (!result?.token_accounts) break

      total += result.token_accounts.length
      cursor = result.cursor || undefined

      // Safety: cap at 50 pages (50k holders) to avoid runaway loops
      if (total > 50_000) break
    } while (cursor)

    // Cache the result
    cache.set(mint, { count: total, expiry: Date.now() + CACHE_TTL_MS })

    // Cleanup stale entries periodically
    if (cache.size > 500) {
      const now = Date.now()
      for (const [key, entry] of cache) {
        if (now > entry.expiry) cache.delete(key)
      }
    }

    return NextResponse.json({ holders: total })
  } catch (error) {
    console.error('Holders count error:', error)
    return NextResponse.json({ error: 'Failed to fetch holders count' }, { status: 500 })
  }
}
