'use client'

import { useState, useEffect, useCallback, useRef } from 'react'
import { PublicKey } from '@solana/web3.js'
import { useConnection } from '@solana/wallet-adapter-react'
import { useNetwork } from '@/lib/NetworkContext'
import { PROGRAM_ID, BONDING_CURVE_SEED } from '@/lib/constants'
import { TokenData } from '@/types/token'

const MEMO_PROGRAM = 'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr'

export type ActionType = 'bought' | 'sold' | 'launched' | 'migrated' | 'messaged' | 'shorted' | 'borrowed'

export interface ActivityEntry {
  trader: string
  token_mint: string
  token_name: string
  action: ActionType
  amount_sol: number | null
  memo: string | null
  timestamp: number
  signature: string
}

function getBondingCurvePda(mint: PublicKey): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from(BONDING_CURVE_SEED), mint.toBuffer()],
    PROGRAM_ID,
  )
  return pda
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function extractMemo(tx: any): string | null {
  const allIxs = [
    ...tx.transaction.message.instructions,
    ...(tx.meta?.innerInstructions || []).flatMap((inner: any) => inner.instructions),
  ]
  for (const ix of allIxs) {
    const pid = 'programId' in ix ? (ix.programId as PublicKey).toString() : ''
    const pname = 'program' in ix ? (ix as { program: string }).program : ''
    if (pid === MEMO_PROGRAM || pname === 'spl-memo') {
      if ('parsed' in ix) {
        return typeof ix.parsed === 'string' ? ix.parsed : JSON.stringify(ix.parsed)
      } else if ('data' in ix && typeof (ix as { data?: string }).data === 'string') {
        const raw = (ix as { data: string }).data
        try {
          return new TextDecoder().decode(Buffer.from(raw, 'base64'))
        } catch {
          return raw
        }
      }
    }
  }
  return null
}

export function useActivityFeed(tokens: TokenData[]) {
  const { connection } = useConnection()
  const { isSimnet } = useNetwork()
  const [entries, setEntries] = useState<ActivityEntry[]>([])
  const [loading, setLoading] = useState(true)
  const fetchingRef = useRef(false)

  const fetchFeed = useCallback(async (showLoading = false) => {
    if (fetchingRef.current || tokens.length === 0) return
    fetchingRef.current = true
    if (showLoading) setLoading(true)

    try {
      const results: ActivityEntry[] = []

      // Take the 15 most recently active tokens
      const sorted = [...tokens]
        .sort((a, b) => (b.last_activity_at || 0) - (a.last_activity_at || 0))
        .slice(0, 15)

      await Promise.all(
        sorted.map(async (token) => {
          try {
            const mint = new PublicKey(token.mint)
            const bondingCurve = getBondingCurvePda(mint)
            const bcAddress = bondingCurve.toString()

            const signatures = await connection.getSignaturesForAddress(
              bondingCurve,
              { limit: 15 },
              'confirmed',
            )

            if (signatures.length === 0) return

            const txs = await connection.getParsedTransactions(
              signatures.map((s) => s.signature),
              { maxSupportedTransactionVersion: 0 },
            )

            for (let i = 0; i < txs.length; i++) {
              const tx = txs[i]
              const sig = signatures[i]
              if (!tx?.meta || tx.meta.err) continue

              const accountKeys = tx.transaction.message.accountKeys
              const bcIndex = accountKeys.findIndex((k) => k.pubkey.toString() === bcAddress)
              if (bcIndex === -1) continue

              const trader = accountKeys[0]?.pubkey?.toString() || ''
              const solChange = tx.meta.postBalances[bcIndex] - tx.meta.preBalances[bcIndex]
              let absSol = Math.abs(solChange) / 1_000_000_000

              const memo = extractMemo(tx)

              // Token balance changes for action detection
              const pre = tx.meta.preTokenBalances || []
              const post = tx.meta.postTokenBalances || []

              let traderTokenDelta = 0
              for (const postBal of post) {
                if (postBal.mint !== token.mint) continue
                if (postBal.owner !== trader) continue
                const preBal = pre.find((p) => p.accountIndex === postBal.accountIndex)
                const preAmt = Number(preBal?.uiTokenAmount?.amount || '0')
                const postAmt = Number(postBal.uiTokenAmount?.amount || '0')
                traderTokenDelta = postAmt - preAmt
                break
              }

              // Vault-routed: check non-signer token delta
              let vaultTokenDelta = 0
              let vaultOwner: string | null = null
              for (const postBal of post) {
                if (postBal.mint !== token.mint) continue
                if (postBal.owner === trader || postBal.owner === bcAddress) continue
                const preBal = pre.find((p) => p.accountIndex === postBal.accountIndex)
                const preAmt = Number(preBal?.uiTokenAmount?.amount || '0')
                const postAmt = Number(postBal.uiTokenAmount?.amount || '0')
                const delta = postAmt - preAmt
                if (delta !== 0) {
                  vaultTokenDelta = delta
                  vaultOwner = postBal.owner ?? null
                  break
                }
              }

              // Vault SOL
              if (vaultOwner) {
                const vaultIndex = accountKeys.findIndex((k) => k.pubkey.toString() === vaultOwner)
                if (vaultIndex !== -1) {
                  const vaultSolDelta = Math.abs(tx.meta.postBalances[vaultIndex] - tx.meta.preBalances[vaultIndex]) / 1_000_000_000
                  if (vaultSolDelta > absSol) absSol = vaultSolDelta
                }
              }

              // Determine action
              const isCreateTx = i === txs.length - 1
              const postBalance = tx.meta.postBalances[bcIndex]
              const isMigrationTx = solChange < 0 && postBalance < 1_000_000_000 && token.status === 'migrated'

              let action: ActionType
              if (isCreateTx) {
                action = 'launched'
              } else if (isMigrationTx) {
                action = 'migrated'
              } else if (traderTokenDelta > 0 || vaultTokenDelta > 0) {
                action = 'bought'
              } else if (traderTokenDelta < 0 || vaultTokenDelta < 0) {
                action = 'sold'
              } else if (memo && (absSol < 0.002)) {
                action = 'messaged'
              } else {
                action = solChange > 0 ? 'bought' : 'sold'
              }

              results.push({
                trader,
                token_mint: token.mint,
                token_name: token.name,
                action,
                amount_sol: absSol > 0.001 ? absSol : null,
                memo,
                timestamp: sig.blockTime || 0,
                signature: sig.signature,
              })
            }
          } catch {
            // skip tokens with errors
          }
        }),
      )

      // Dedupe by signature, sort newest first
      const bySignature = new Map<string, ActivityEntry>()
      for (const e of results) {
        if (!bySignature.has(e.signature)) {
          bySignature.set(e.signature, e)
        }
      }

      const sorted2 = Array.from(bySignature.values()).sort((a, b) => b.timestamp - a.timestamp)
      setEntries(sorted2)
    } catch {
      // ignore
    } finally {
      fetchingRef.current = false
      setLoading(false)
    }
  }, [connection, tokens])

  // Initial fetch when tokens are loaded
  useEffect(() => {
    if (tokens.length > 0) {
      fetchFeed(true)
    }
  }, [tokens.length > 0]) // eslint-disable-line react-hooks/exhaustive-deps

  // Live updates
  useEffect(() => {
    if (tokens.length === 0) return

    if (isSimnet) {
      const interval = setInterval(() => fetchFeed(), 5000)
      return () => clearInterval(interval)
    }

    const subId = connection.onProgramAccountChange(
      PROGRAM_ID,
      () => { fetchFeed() },
      {
        commitment: 'confirmed',
        filters: [{ memcmp: { offset: 0, bytes: '4y6pru6YvC7' } }],
      },
    )

    return () => { connection.removeProgramAccountChangeListener(subId) }
  }, [connection, isSimnet, fetchFeed, tokens.length])

  return { entries, loading }
}
