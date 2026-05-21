'use client'

import { useState } from 'react'
import Link from 'next/link'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { buildCreateVaultTransaction } from 'torchsdk'
import { Header, HowItWorksModal, TreasuryModal } from '@/components'
import { useVault } from '@/hooks/useVault'
import { VaultDashboard } from '@/components/vault/VaultDashboard'
import { VaultActions } from '@/components/vault/VaultActions'
import { VaultTokens } from '@/components/vault/VaultTokens'
import { VaultWallets } from '@/components/vault/VaultWallets'
import { VaultAuthority } from '@/components/vault/VaultAuthority'

export default function VaultPage() {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()
  const { vault, linkedVault, activeVault, loading, refetch } = useVault()

  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)
  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [showTreasuryModal, setShowTreasuryModal] = useState(false)

  async function handleCreateVault() {
    if (!wallet.publicKey) return

    setCreating(true)
    setCreateError(null)

    try {
      const { transaction: tx } = await buildCreateVaultTransaction(connection, {
        creator: wallet.publicKey.toString(),
      })

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      // Refetch vault state
      setTimeout(refetch, 1000)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to create vault'
      if (msg.includes('User rejected')) {
        setCreateError('Transaction cancelled')
      } else {
        setCreateError(msg)
      }
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header
        onHowItWorksClick={() => setShowHowItWorks(true)}
        onTreasuryClick={() => setShowTreasuryModal(true)}
      />

      <main className="px-4 sm:px-6 lg:px-8 pb-16">
        <div className="max-w-2xl mx-auto pt-2">
          <Link
            href="/"
            className="text-sm mb-4 inline-block transition-colors"
            style={{ color: 'var(--muted)' }}
          >
            ← portfolio
          </Link>
          <h1
            className="text-2xl font-bold tracking-tight lowercase mb-1"
            style={{ color: 'var(--foreground)' }}
          >
            vault
          </h1>
          <p className="text-sm mb-8" style={{ color: 'var(--muted)' }}>
            your on-chain SOL vault for automated trading, agent wallets, and custody.
          </p>

          {!wallet.publicKey ? (
            <div className="text-center py-16">
              <p style={{ color: 'var(--muted)' }}>connect your wallet to manage your vault.</p>
            </div>
          ) : loading ? (
            <div className="text-center py-16">
              <p style={{ color: 'var(--muted)' }}>loading vault…</p>
            </div>
          ) : !activeVault ? (
            <div className="text-center py-16 max-w-md mx-auto">
              <h2
                className="text-lg font-semibold lowercase mb-2"
                style={{ color: 'var(--foreground)' }}
              >
                no vault yet
              </h2>
              <p className="text-sm mb-6" style={{ color: 'var(--muted)' }}>
                create a vault to deposit SOL, link agent wallets, and route trades through a
                controlled on-chain account.
              </p>
              {createError && <p className="text-danger text-sm mb-4">{createError}</p>}
              <button
                onClick={handleCreateVault}
                disabled={creating}
                className="btn btn-accent px-8 py-3 disabled:opacity-50"
              >
                {creating ? 'creating…' : 'create vault'}
              </button>
            </div>
          ) : (
            <div className="space-y-4">
              {!vault && linkedVault && (
                <p className="text-sm" style={{ color: 'var(--muted)' }}>
                  viewing a vault you are linked to (not your own).
                </p>
              )}
              <VaultDashboard vault={activeVault} />
              <VaultActions vault={activeVault} onSuccess={refetch} />
              <VaultTokens vault={activeVault} onSuccess={refetch} />
              <VaultWallets vault={activeVault} onSuccess={refetch} />
              <VaultAuthority vault={activeVault} onSuccess={refetch} />
            </div>
          )}
        </div>
      </main>

      <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
      <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
    </div>
  )
}
