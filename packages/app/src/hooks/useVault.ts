'use client'

import { useState, useEffect, useCallback } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { getVault, getVaultForWallet } from 'torchsdk'
import type { VaultInfo } from 'torchsdk'

const isDev = process.env.NODE_ENV === 'development'

export type { VaultInfo }

interface UseVaultResult {
  /** Vault owned by the connected wallet (creator = wallet) */
  vault: VaultInfo | null
  /** Vault the wallet is linked to (if it's an agent/linked wallet) */
  linkedVault: VaultInfo | null
  /** The active vault (own vault takes priority, then linked) */
  activeVault: VaultInfo | null
  loading: boolean
  error: string | null
  refetch: () => void
}

export function useVault(): UseVaultResult {
  const { connection } = useConnection()
  const { publicKey } = useWallet()

  const [vault, setVault] = useState<VaultInfo | null>(null)
  const [linkedVault, setLinkedVault] = useState<VaultInfo | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const fetchVault = useCallback(async () => {
    if (!publicKey) {
      setVault(null)
      setLinkedVault(null)
      return
    }

    setLoading(true)
    setError(null)

    try {
      const walletStr = publicKey.toString()

      // Fetch own vault and linked vault in parallel
      const [ownVault, linked] = await Promise.all([
        getVault(connection, walletStr).catch(() => null),
        getVaultForWallet(connection, walletStr).catch(() => null),
      ])

      setVault(ownVault)
      // Only set linkedVault if it's different from own vault
      setLinkedVault(linked && linked.address !== ownVault?.address ? linked : null)
    } catch (err) {
      if (isDev) console.error('Failed to fetch vault:', err)
      setError(err instanceof Error ? err.message : 'Failed to fetch vault')
    } finally {
      setLoading(false)
    }
  }, [connection, publicKey])

  useEffect(() => {
    fetchVault()
  }, [fetchVault])

  // Own vault takes priority, then linked vault
  const activeVault = vault || linkedVault || null

  return {
    vault,
    linkedVault,
    activeVault,
    loading,
    error,
    refetch: fetchVault,
  }
}
