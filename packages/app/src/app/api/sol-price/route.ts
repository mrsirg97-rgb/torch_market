import { NextResponse } from 'next/server'

let cachedPrice: { usd: number; ts: number } | null = null
const CACHE_TTL = 60_000 // 1 minute

export async function GET() {
  if (cachedPrice && Date.now() - cachedPrice.ts < CACHE_TTL) {
    return NextResponse.json({ solana: { usd: cachedPrice.usd } })
  }

  try {
    const res = await fetch(
      'https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd',
      { next: { revalidate: 60 } },
    )
    if (!res.ok) {
      if (cachedPrice) {
        return NextResponse.json({ solana: { usd: cachedPrice.usd } })
      }
      return NextResponse.json({ solana: { usd: null } }, { status: 502 })
    }
    const data = await res.json()
    const usd = data?.solana?.usd
    if (typeof usd === 'number' && usd > 0) {
      cachedPrice = { usd, ts: Date.now() }
    }
    return NextResponse.json(data)
  } catch {
    if (cachedPrice) {
      return NextResponse.json({ solana: { usd: cachedPrice.usd } })
    }
    return NextResponse.json({ solana: { usd: null } }, { status: 502 })
  }
}
