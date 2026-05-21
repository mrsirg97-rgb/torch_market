import { NextRequest, NextResponse } from 'next/server'

const SAID_API_URL = 'https://api.saidprotocol.com/api'

// Circuit breaker: when upstream is unreachable (cert mismatch, network failure),
// stop trying for a window so we don't fire one fetch per wallet and flood the log.
// Per-process in-memory state — resets on hot reload in dev, persists in prod single-instance.
let circuitOpenUntil = 0
let lastLogAt = 0
const CIRCUIT_OPEN_MS = 60_000
const LOG_THROTTLE_MS = 60_000

function compactErr(err: unknown): string {
  if (err instanceof Error) {
    const cause = (err as Error & { cause?: { code?: string; reason?: string } }).cause
    if (cause?.code) return cause.reason ? `${cause.code}: ${cause.reason}` : cause.code
    return err.message
  }
  return String(err)
}

const DEGRADED_BODY = { verified: false }
const DEGRADED_HEADERS = { 'Cache-Control': 'public, s-maxage=60' }

export async function GET(
  _request: NextRequest,
  { params }: { params: Promise<{ wallet: string }> },
) {
  const { wallet } = await params
  const now = Date.now()

  if (now < circuitOpenUntil) {
    return NextResponse.json(DEGRADED_BODY, { status: 200, headers: DEGRADED_HEADERS })
  }

  try {
    const res = await fetch(`${SAID_API_URL}/verify/${wallet}`, {
      next: { revalidate: 300 },
    })
    const data = await res.json()
    return NextResponse.json(data, {
      headers: { 'Cache-Control': 'public, s-maxage=300, stale-while-revalidate=600' },
    })
  } catch (err) {
    circuitOpenUntil = now + CIRCUIT_OPEN_MS
    if (now - lastLogAt > LOG_THROTTLE_MS) {
      lastLogAt = now
      console.warn(
        `[said-verify] upstream unavailable — circuit open ${CIRCUIT_OPEN_MS / 1000}s: ${compactErr(err)}`,
      )
    }
    return NextResponse.json(DEGRADED_BODY, { status: 200, headers: DEGRADED_HEADERS })
  }
}
