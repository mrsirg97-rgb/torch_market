'use client'

import { useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { PublicKey } from '@solana/web3.js'
import { buildTransferAuthorityTransaction } from 'torchsdk'
import type { VaultInfo } from 'torchsdk'
import { shortenAddress } from '@/lib/constants'

interface VaultAuthorityProps {
  vault: VaultInfo
  onSuccess: () => void
}

export function VaultAuthority({ vault, onSuccess }: VaultAuthorityProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const [newAuthority, setNewAuthority] = useState('')
  const [confirmStep, setConfirmStep] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const isAuthority = wallet.publicKey?.toString() === vault.authority

  async function handleTransfer() {
    if (!wallet.publicKey || !isAuthority) return

    const addr = newAuthority.trim()
    try {
      new PublicKey(addr)
    } catch {
      setError('Invalid wallet address')
      return
    }

    if (addr === wallet.publicKey.toString()) {
      setError('New authority must be different from current')
      return
    }

    setLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction: tx } = await buildTransferAuthorityTransaction(connection, {
        authority: wallet.publicKey.toString(),
        vault_creator: vault.creator,
        new_authority: addr,
      })

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setNewAuthority('')
      setConfirmStep(false)
      setSuccess(`Authority transferred to ${shortenAddress(addr)}`)
      setTimeout(onSuccess, 1500)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to transfer authority'
      setError(msg.includes('User rejected') ? 'Transaction cancelled' : msg)
    } finally {
      setLoading(false)
    }
  }

  if (!isAuthority) return null

  return (
    <div className="card border border-danger/20 rounded-xl p-4">
      <h3 className="text-sm font-semibold text-danger/80 mb-1">Transfer Authority</h3>
      <p className="text-xs text-white/40 mb-3">
        Transfer vault control to another wallet. This action is irreversible.
      </p>

      <div className="space-y-3">
        <div>
          <label className="text-xs text-white/50 mb-1 block">New Authority</label>
          <input
            type="text"
            value={newAuthority}
            onChange={(e) => { setNewAuthority(e.target.value); setConfirmStep(false) }}
            placeholder="New authority wallet address..."
            className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-xs text-white placeholder:text-white/30 focus:outline-none focus:border-danger/50 transition-colors"
          />
        </div>

        {error && <p className="text-xs text-danger">{error}</p>}
        {success && <p className="text-xs text-success">{success}</p>}

        {!confirmStep ? (
          <button
            onClick={() => {
              if (!newAuthority.trim()) {
                setError('Enter a wallet address')
                return
              }
              try {
                new PublicKey(newAuthority.trim())
              } catch {
                setError('Invalid wallet address')
                return
              }
              setError(null)
              setConfirmStep(true)
            }}
            disabled={!newAuthority.trim()}
            className="w-full py-2 text-xs rounded-lg font-medium bg-danger/20 text-danger hover:bg-danger/30 transition-colors cursor-pointer disabled:opacity-50"
          >
            Transfer Authority
          </button>
        ) : (
          <div className="space-y-2">
            <div className="bg-danger/10 border border-danger/20 rounded-lg p-2">
              <p className="text-xs text-danger font-medium">Are you sure?</p>
              <p className="text-xs text-white/40 mt-1">
                This will transfer full control to {shortenAddress(newAuthority.trim())}. You will lose access.
              </p>
            </div>
            <div className="grid grid-cols-2 gap-2">
              <button
                onClick={() => setConfirmStep(false)}
                className="py-2 text-xs rounded-lg bg-white/10 text-white/70 hover:bg-white/20 transition-colors cursor-pointer"
              >
                Cancel
              </button>
              <button
                onClick={handleTransfer}
                disabled={loading}
                className="py-2 text-xs rounded-lg bg-danger text-white font-medium hover:bg-danger/80 transition-colors cursor-pointer disabled:opacity-50"
              >
                {loading ? 'Transferring...' : 'Confirm Transfer'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
