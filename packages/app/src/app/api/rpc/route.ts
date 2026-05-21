import { NextRequest, NextResponse } from 'next/server'

// Server-side only - this key is never exposed to the client
const HELIUS_API_KEY = process.env.HELIUS_API_KEY
const HELIUS_RPC_URL = `https://mainnet.helius-rpc.com/?api-key=${HELIUS_API_KEY}`

// Fallback URLs for non-mainnet or if Helius key not configured
const RPC_URLS: Record<string, string> = {
  mainnet: HELIUS_API_KEY ? HELIUS_RPC_URL : 'https://api.mainnet-beta.solana.com',
  devnet: 'https://api.devnet.solana.com',
  local: 'http://localhost:8899',
}

// Allowlisted RPC methods — read-only operations only.
// Blocks sendTransaction, simulateTransaction, requestAirdrop, and other write/dangerous methods.
const ALLOWED_RPC_METHODS = new Set([
  'getAccountInfo',
  'getBalance',
  'getBlock',
  'getBlockHeight',
  'getBlockTime',
  'getConfirmedBlock',
  'getEpochInfo',
  'getEpochSchedule',
  'getFeeForMessage',
  'getFirstAvailableBlock',
  'getGenesisHash',
  'getHealth',
  'getHighestSnapshotSlot',
  'getIdentity',
  'getInflationGovernor',
  'getInflationRate',
  'getLatestBlockhash',
  'getLeaderSchedule',
  'getMinimumBalanceForRentExemption',
  'getMultipleAccounts',
  'getProgramAccounts',
  'getRecentBlockhash',
  'getRecentPerformanceSamples',
  'getSignatureStatuses',
  'getSignaturesForAddress',
  'getSlot',
  'getSlotLeader',
  'getStakeMinimumDelegation',
  'getSupply',
  'getTokenAccountBalance',
  'getTokenAccountsByOwner',
  'getTokenLargestAccounts',
  'getTokenSupply',
  'getTransaction',
  'getTransactionCount',
  'getVersion',
  'getVoteAccounts',
  'isBlockhashValid',
])

export async function POST(request: NextRequest) {
  try {
    const body = await request.json()

    // Validate RPC method is allowlisted
    const method = body?.method
    if (!method || typeof method !== 'string' || !ALLOWED_RPC_METHODS.has(method)) {
      return NextResponse.json({ error: `RPC method not allowed: ${method}` }, { status: 403 })
    }

    const network = process.env.NEXT_PUBLIC_NETWORK || 'devnet'
    const rpcUrl = RPC_URLS[network] || RPC_URLS.devnet

    const response = await fetch(rpcUrl, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    })

    const data = await response.json()
    return NextResponse.json(data)
  } catch (error) {
    console.error('RPC proxy error:', error)
    return NextResponse.json({ error: 'RPC request failed' }, { status: 500 })
  }
}
