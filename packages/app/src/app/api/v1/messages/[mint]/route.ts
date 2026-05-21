import { NextRequest, NextResponse } from 'next/server'
import { Connection, PublicKey } from '@solana/web3.js'
import { getBondingCurvePda, getDeepPoolAccounts } from 'torchsdk'

// Same Cloudflare Worker proxy the client uses — Helius API key baked in
const HELIUS_PROXY_URL = 'https://torch-market-rpc.mrsirg97.workers.dev'

const MEMO_PROGRAMS = new Set([
  'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr',
  'Memo1UhkJBfCR6MNB9RLUt3uh6YLPUmhAP3zCs86mST',
])

interface CachedMessage {
  signature: string
  memo: string
  sender: string
  timestamp: number
}

interface CachedMessages {
  messages: CachedMessage[]
  newestSignature: string | null
  fullyLoaded: boolean
  lastRefresh: number
}

const cache = new Map<string, CachedMessages>()
const CACHE_TTL_MS = 30_000 // 30 seconds
const MAX_CACHE_ENTRIES = 200
const MAX_PAGES = 20
const SIGS_PER_PAGE = 1000
const TX_BATCH_SIZE = 100

const connection = new Connection(HELIUS_PROXY_URL, 'confirmed')

function deriveBondingCurvePda(mint: PublicKey): PublicKey {
  return getBondingCurvePda(mint)[0]
}

function deriveDeepPoolPda(mint: PublicKey): PublicKey {
  return getDeepPoolAccounts(mint).pool
}

// Strip "[123] " prefix that Solana RPC adds to memo fields
function cleanMemo(raw: string): string {
  return raw.replace(/^\[\d+\]\s*/, '').trim()
}

interface SigInfo {
  signature: string
  memo: string | null
  blockTime: number | null
}

async function fetchSignatures(
  address: PublicKey,
  options?: { until?: string },
): Promise<{ sigs: SigInfo[]; reachedEnd: boolean }> {
  const allSigs: SigInfo[] = []
  let before: string | undefined

  for (let page = 0; page < MAX_PAGES; page++) {
    const params: { limit: number; before?: string; until?: string } = {
      limit: SIGS_PER_PAGE,
    }
    if (before) params.before = before
    if (options?.until) params.until = options.until

    const sigs = await connection.getSignaturesForAddress(address, params, 'confirmed')
    if (sigs.length === 0) return { sigs: allSigs, reachedEnd: true }

    for (const sig of sigs) {
      if (!sig.err) {
        allSigs.push({
          signature: sig.signature,
          memo: sig.memo,
          blockTime: sig.blockTime ?? null,
        })
      }
    }

    if (sigs.length < SIGS_PER_PAGE) return { sigs: allSigs, reachedEnd: true }
    before = sigs[sigs.length - 1].signature
  }

  return { sigs: allSigs, reachedEnd: false }
}

// Fast path: Helius populates the memo field on getSignaturesForAddress.
// We only need getParsedTransactions for the subset with memos (sender extraction).
async function extractFromMemoField(
  sigs: SigInfo[],
): Promise<CachedMessage[]> {
  const sigsWithMemo = sigs.filter((s) => s.memo)
  if (sigsWithMemo.length === 0) return []

  // Batch-fetch parsed transactions only for sigs with memos (for sender)
  const senderMap = new Map<string, string>()
  for (let i = 0; i < sigsWithMemo.length; i += TX_BATCH_SIZE) {
    const batch = sigsWithMemo.slice(i, i + TX_BATCH_SIZE)
    try {
      const txs = await connection.getParsedTransactions(
        batch.map((s) => s.signature),
        { maxSupportedTransactionVersion: 0 },
      )
      for (let j = 0; j < txs.length; j++) {
        const tx = txs[j]
        if (tx?.transaction?.message?.accountKeys?.[0]) {
          senderMap.set(batch[j].signature, tx.transaction.message.accountKeys[0].pubkey.toString())
        }
      }
    } catch {
      // Batch failed — senders fall back to 'Unknown'
    }
  }

  return sigsWithMemo.map((s) => ({
    signature: s.signature,
    memo: cleanMemo(s.memo!),
    sender: senderMap.get(s.signature) || 'Unknown',
    timestamp: s.blockTime || 0,
  }))
}

// Slow fallback: parse full transactions for SPL Memo instructions.
// Used if the RPC doesn't populate the memo field on signatures.
async function extractFromTransactions(
  sigs: SigInfo[],
): Promise<CachedMessage[]> {
  const messages: CachedMessage[] = []

  for (let i = 0; i < sigs.length; i += TX_BATCH_SIZE) {
    const batch = sigs.slice(i, i + TX_BATCH_SIZE)
    try {
      const txs = await connection.getParsedTransactions(
        batch.map((s) => s.signature),
        { maxSupportedTransactionVersion: 0 },
      )
      for (let j = 0; j < txs.length; j++) {
        const tx = txs[j]
        if (!tx?.meta || tx.meta.err) continue

        for (const ix of tx.transaction.message.instructions) {
          const programId = 'programId' in ix ? ix.programId.toString() : ''
          const programName = 'program' in ix ? (ix as { program: string }).program : ''
          if (!MEMO_PROGRAMS.has(programId) && programName !== 'spl-memo') continue

          let memoText = ''
          if ('parsed' in ix) {
            memoText = typeof ix.parsed === 'string' ? ix.parsed : JSON.stringify(ix.parsed)
          } else if ('data' in ix && typeof ix.data === 'string') {
            try {
              memoText = Buffer.from(ix.data, 'base64').toString('utf-8')
            } catch {
              memoText = ix.data
            }
          }

          if (memoText && memoText.trim()) {
            const sender =
              tx.transaction.message.accountKeys[0]?.pubkey?.toString() || 'Unknown'
            messages.push({
              signature: batch[j].signature,
              memo: cleanMemo(memoText.trim()),
              sender,
              timestamp: batch[j].blockTime || 0,
            })
            break
          }
        }
      }
    } catch {
      // Batch failed, skip
    }
  }

  return messages
}

// Try memo-field fast path; fall back to full transaction parsing
async function extractMessages(sigs: SigInfo[]): Promise<CachedMessage[]> {
  const hasMemoFields = sigs.some((s) => s.memo)
  if (hasMemoFields) return extractFromMemoField(sigs)
  return extractFromTransactions(sigs)
}

function evictStaleEntries() {
  if (cache.size <= MAX_CACHE_ENTRIES) return
  const now = Date.now()
  for (const [key, entry] of cache) {
    if (now - entry.lastRefresh > CACHE_TTL_MS) cache.delete(key)
  }
}

export async function GET(
  _request: NextRequest,
  { params }: { params: Promise<{ mint: string }> },
) {
  const { mint } = await params

  // Validate mint (base58, 32-44 chars)
  if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(mint)) {
    return NextResponse.json({ error: 'Invalid mint address' }, { status: 400 })
  }

  // Return cached if fresh
  const cached = cache.get(mint)
  if (cached && Date.now() - cached.lastRefresh < CACHE_TTL_MS) {
    return NextResponse.json({ messages: cached.messages, total: cached.messages.length })
  }

  try {
    const mintPubkey = new PublicKey(mint)
    const bondingCurvePda = deriveBondingCurvePda(mintPubkey)
    const deepPoolPda = deriveDeepPoolPda(mintPubkey)

    let messages: CachedMessage[]
    let newestSignature: string | null
    let fullyLoaded: boolean

    if (cached) {
      // Incremental refresh — fetch only newer signatures from both sources
      const [bondingResult, deepPoolResult] = await Promise.all([
        fetchSignatures(bondingCurvePda, { until: cached.newestSignature! }),
        fetchSignatures(deepPoolPda, { until: cached.newestSignature! }).catch(() => ({ sigs: [] as SigInfo[], reachedEnd: true })),
      ])

      const allNewSigs = [...bondingResult.sigs, ...deepPoolResult.sigs]
      const newMessages = await extractMessages(allNewSigs)

      messages = [...newMessages, ...cached.messages]
      // Deduplicate by signature
      const seen = new Set<string>()
      messages = messages.filter((m) => {
        if (seen.has(m.signature)) return false
        seen.add(m.signature)
        return true
      })
      newestSignature = allNewSigs.length > 0 ? allNewSigs[0].signature : cached.newestSignature
      fullyLoaded = cached.fullyLoaded
    } else {
      // Full backfill — paginate all signatures from both sources
      const [bondingResult, deepPoolResult] = await Promise.all([
        fetchSignatures(bondingCurvePda),
        fetchSignatures(deepPoolPda).catch(() => ({ sigs: [] as SigInfo[], reachedEnd: true })),
      ])

      const allSigs = [...bondingResult.sigs, ...deepPoolResult.sigs]
      fullyLoaded = bondingResult.reachedEnd && deepPoolResult.reachedEnd

      messages = await extractMessages(allSigs)
      // Deduplicate by signature
      const seen = new Set<string>()
      messages = messages.filter((m) => {
        if (seen.has(m.signature)) return false
        seen.add(m.signature)
        return true
      })
      // Sort by timestamp descending (newest first)
      messages.sort((a, b) => b.timestamp - a.timestamp)
      newestSignature = allSigs.length > 0 ? allSigs[0].signature : null
    }

    cache.set(mint, { messages, newestSignature, fullyLoaded, lastRefresh: Date.now() })
    evictStaleEntries()

    return NextResponse.json({ messages, total: messages.length })
  } catch (error) {
    console.error('Messages fetch error:', error)
    return NextResponse.json({ error: 'Failed to fetch messages' }, { status: 500 })
  }
}
