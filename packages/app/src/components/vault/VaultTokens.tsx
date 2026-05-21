'use client'

import { useState, useEffect, useMemo } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { PublicKey } from '@solana/web3.js'
import { TOKEN_2022_PROGRAM_ID, getAssociatedTokenAddressSync } from '@solana/spl-token'
import { buildWithdrawTokensTransaction, getTokens, getTorchVaultPda } from 'torchsdk'
import type { VaultInfo } from 'torchsdk'
import { formatTokens, shortenAddress } from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'

interface VaultTokenHolding {
  mint: string
  symbol: string
  name: string
  balance: bigint
}

interface VaultTokensProps {
  vault: VaultInfo
  onSuccess: () => void
}

export function VaultTokens({ vault, onSuccess }: VaultTokensProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const [holdings, setHoldings] = useState<VaultTokenHolding[]>([])
  const [loading, setLoading] = useState(true)
  const [withdrawMint, setWithdrawMint] = useState<string | null>(null)
  const [withdrawAmount, setWithdrawAmount] = useState('')
  const [withdrawLoading, setWithdrawLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const isAuthority = wallet.publicKey?.toString() === vault.authority
  const vaultPda = useMemo(
    () => getTorchVaultPda(new PublicKey(vault.creator))[0],
    [vault.creator],
  )

  // Fetch vault token holdings
  useEffect(() => {
    let cancelled = false

    async function fetchHoldings() {
      setLoading(true)
      try {
        // Get all tokens to know which mints exist
        const { tokens } = await getTokens(connection)
        if (cancelled) return

        // Build ATAs for vault PDA
        const atas = tokens.map((t) => ({
          mint: t.mint,
          symbol: t.symbol,
          name: t.name,
          ata: getAssociatedTokenAddressSync(
            new PublicKey(t.mint),
            vaultPda,
            true, // allowOwnerOffCurve for PDA
            TOKEN_2022_PROGRAM_ID,
          ),
        }))

        // Batch fetch (up to 100 at a time)
        const results: VaultTokenHolding[] = []
        for (let i = 0; i < atas.length; i += 100) {
          const batch = atas.slice(i, i + 100)
          const infos = await connection.getMultipleAccountsInfo(batch.map((a) => a.ata))
          if (cancelled) return

          infos.forEach((info, idx) => {
            if (info && info.data.length >= 72) {
              const balance = info.data.readBigUInt64LE(64)
              if (balance > BigInt(0)) {
                results.push({
                  mint: batch[idx].mint,
                  symbol: batch[idx].symbol,
                  name: batch[idx].name,
                  balance,
                })
              }
            }
          })
        }

        // Sort by balance descending
        results.sort((a, b) => (b.balance > a.balance ? 1 : b.balance < a.balance ? -1 : 0))
        setHoldings(results)
      } catch (err) {
        if (isDev) console.error('Failed to fetch vault token holdings:', err)
      } finally {
        if (!cancelled) setLoading(false)
      }
    }

    fetchHoldings()
    return () => { cancelled = true }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connection, vault.creator])

  async function handleWithdraw(mint: string) {
    if (!wallet.publicKey || !isAuthority) return

    const holding = holdings.find((h) => h.mint === mint)
    if (!holding) return

    const tokenAmount = parseFloat(withdrawAmount)
    if (isNaN(tokenAmount) || tokenAmount <= 0) {
      setError('Enter a valid amount')
      return
    }

    const units = Math.floor(tokenAmount * 1_000_000) // 6 decimals
    if (BigInt(units) > holding.balance) {
      setError('Exceeds vault balance')
      return
    }

    setWithdrawLoading(true)
    setError(null)
    setSuccess(null)

    try {
      // Destination is the user's ATA
      const destination = getAssociatedTokenAddressSync(
        new PublicKey(mint),
        wallet.publicKey,
        false,
        TOKEN_2022_PROGRAM_ID,
      )

      const { transaction: tx } = await buildWithdrawTokensTransaction(connection, {
        authority: wallet.publicKey.toString(),
        vault_creator: vault.creator,
        mint,
        destination: destination.toString(),
        amount: units,
      })

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setWithdrawMint(null)
      setWithdrawAmount('')
      setSuccess(`Withdrew ${formatTokens(units)} tokens`)
      setTimeout(onSuccess, 1500)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to withdraw tokens'
      setError(msg.includes('User rejected') ? 'Transaction cancelled' : msg)
    } finally {
      setWithdrawLoading(false)
    }
  }

  return (
    <div className="card border border-white/10 rounded-xl p-4">
      <h3 className="text-sm font-semibold text-white/80 mb-3">market positions</h3>

      {loading ? (
        <p className="text-xs text-white/40">loading positions…</p>
      ) : holdings.length === 0 ? (
        <p className="text-xs text-white/40">No tokens in vault</p>
      ) : (
        <div className="space-y-2">
          {holdings.map((h) => (
            <div key={h.mint} className="bg-white/5 rounded-lg p-3">
              <div className="flex items-center justify-between">
                <div>
                  <p className="text-sm text-white font-medium">${h.symbol}</p>
                  <p className="text-xs text-white/40">{shortenAddress(h.mint)}</p>
                </div>
                <div className="text-right">
                  <p className="text-sm text-white">{formatTokens(h.balance)}</p>
                  {isAuthority && (
                    <button
                      onClick={() => {
                        setWithdrawMint(withdrawMint === h.mint ? null : h.mint)
                        setWithdrawAmount('')
                        setError(null)
                        setSuccess(null)
                      }}
                      className="text-xs text-accent hover:text-accent/80 cursor-pointer mt-1"
                    >
                      {withdrawMint === h.mint ? 'Cancel' : 'Withdraw'}
                    </button>
                  )}
                </div>
              </div>

              {withdrawMint === h.mint && (
                <div className="mt-3 pt-3 border-t border-white/10">
                  <div className="flex gap-2">
                    <input
                      type="number"
                      value={withdrawAmount}
                      onChange={(e) => setWithdrawAmount(e.target.value)}
                      placeholder="Amount..."
                      className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-xs text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors min-w-0"
                    />
                    <button
                      onClick={() => {
                        setWithdrawAmount((Number(h.balance) / 1_000_000).toString())
                      }}
                      className="px-2 py-2 text-xs bg-white/10 rounded-lg text-white/70 hover:bg-white/20 cursor-pointer"
                    >
                      Max
                    </button>
                    <button
                      onClick={() => handleWithdraw(h.mint)}
                      disabled={withdrawLoading}
                      className="px-3 py-2 text-xs font-medium rounded-lg bg-accent/20 text-accent hover:bg-accent/30 transition-colors cursor-pointer disabled:opacity-50"
                    >
                      {withdrawLoading ? '...' : 'Send'}
                    </button>
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {error && <p className="text-xs text-danger mt-2">{error}</p>}
      {success && <p className="text-xs text-success mt-2">{success}</p>}
    </div>
  )
}
