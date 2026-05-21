'use client'

import { useState } from 'react'
import Link from 'next/link'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { buildCreateVaultTransaction } from 'torchsdk'
import { useVault } from '@/hooks/useVault'

/**
 * Inline vault summary on the portfolio page.
 * - No wallet → render nothing
 * - No vault → show "create vault" CTA
 * - Vault → show SOL balance + link to /vault
 */
export function VaultStrip() {
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const sendTransaction = useMwaSendTransaction()
  const { activeVault, loading, refetch } = useVault()
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  if (!publicKey) return null
  if (loading) {
    return (
      <p className="text-sm py-6" style={{ color: 'var(--muted)' }}>
        loading vault…
      </p>
    )
  }

  async function handleCreate() {
    if (!publicKey) return
    setCreating(true)
    setError(null)
    try {
      const { transaction } = await buildCreateVaultTransaction(connection, {
        creator: publicKey.toString(),
      })
      const txId = await sendTransaction(transaction)
      await connection.confirmTransaction(txId, 'confirmed')
      setTimeout(refetch, 1000)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'failed to create vault'
      setError(msg.includes('User rejected') ? 'cancelled' : msg)
    } finally {
      setCreating(false)
    }
  }

  if (!activeVault) {
    return (
      <div className="card p-4 flex items-center justify-between gap-4">
        <div>
          <p className="text-sm font-medium" style={{ color: 'var(--foreground)' }}>
            no vault yet
          </p>
          <p className="text-xs mt-0.5" style={{ color: 'var(--muted)' }}>
            optional — route trades through an on-chain SOL vault for agent wallets and custody.
          </p>
          {error && <p className="text-xs text-danger mt-1">{error}</p>}
        </div>
        <button
          onClick={handleCreate}
          disabled={creating}
          className="btn btn-accent text-sm whitespace-nowrap cursor-pointer disabled:opacity-50"
        >
          {creating ? 'creating…' : 'create vault'}
        </button>
      </div>
    )
  }

  return (
    <Link href="/vault" className="block">
      <div className="token-card p-4 flex items-center justify-between gap-4 cursor-pointer">
        <div>
          <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
            vault
          </p>
          <p
            className="text-lg font-mono font-bold mt-0.5"
            style={{ color: 'var(--foreground)' }}
          >
            {activeVault.sol_balance.toFixed(4)} SOL
          </p>
        </div>
        <span className="text-xs" style={{ color: 'var(--muted)' }}>
          manage →
        </span>
      </div>
    </Link>
  )
}
