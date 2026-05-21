'use client'

import { useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { PublicKey } from '@solana/web3.js'
import { buildLinkWalletTransaction, buildUnlinkWalletTransaction, getVaultWalletLink } from 'torchsdk'
import type { VaultInfo } from 'torchsdk'
import { shortenAddress } from '@/lib/constants'

interface VaultWalletsProps {
  vault: VaultInfo
  onSuccess: () => void
}

export function VaultWallets({ vault, onSuccess }: VaultWalletsProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const [walletAddress, setWalletAddress] = useState('')
  const [loading, setLoading] = useState(false)
  const [unlinkLoading, setUnlinkLoading] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // Check wallet address to unlink
  const [checkAddress, setCheckAddress] = useState('')
  const [checkResult, setCheckResult] = useState<{ wallet: string; linked: boolean } | null>(null)
  const [checking, setChecking] = useState(false)

  const isAuthority = wallet.publicKey?.toString() === vault.authority

  async function handleLink() {
    if (!wallet.publicKey || !isAuthority) return

    const addr = walletAddress.trim()
    if (!addr) {
      setError('Enter a wallet address to link')
      return
    }

    try {
      new PublicKey(addr)
    } catch {
      setError('Invalid wallet address')
      return
    }

    setLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const { transaction: tx } = await buildLinkWalletTransaction(connection, {
        authority: wallet.publicKey.toString(),
        vault_creator: vault.creator,
        wallet_to_link: addr,
      })

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setWalletAddress('')
      setSuccess(`Linked ${shortenAddress(addr)}`)
      setTimeout(onSuccess, 1500)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to link wallet'
      setError(msg.includes('User rejected') ? 'Transaction cancelled' : msg)
    } finally {
      setLoading(false)
    }
  }

  async function handleCheckAndUnlink() {
    if (!checkAddress.trim()) {
      setError('Enter a wallet address to check')
      return
    }

    try {
      new PublicKey(checkAddress.trim())
    } catch {
      setError('Invalid wallet address')
      return
    }

    setChecking(true)
    setError(null)

    try {
      const link = await getVaultWalletLink(connection, checkAddress.trim())
      if (link && link.vault === vault.address) {
        setCheckResult({ wallet: checkAddress.trim(), linked: true })
      } else {
        setCheckResult({ wallet: checkAddress.trim(), linked: false })
      }
    } catch {
      setCheckResult({ wallet: checkAddress.trim(), linked: false })
    } finally {
      setChecking(false)
    }
  }

  async function handleUnlink(addr: string) {
    if (!wallet.publicKey || !isAuthority) return

    setUnlinkLoading(addr)
    setError(null)
    setSuccess(null)

    try {
      const { transaction: tx } = await buildUnlinkWalletTransaction(connection, {
        authority: wallet.publicKey.toString(),
        vault_creator: vault.creator,
        wallet_to_unlink: addr,
      })

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setSuccess(`Unlinked ${shortenAddress(addr)}`)
      setCheckResult(null)
      setCheckAddress('')
      setTimeout(onSuccess, 1500)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to unlink wallet'
      setError(msg.includes('User rejected') ? 'Transaction cancelled' : msg)
    } finally {
      setUnlinkLoading(null)
    }
  }

  return (
    <div className="card border border-white/10 rounded-xl p-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-semibold text-white/80">Linked Wallets</h3>
        <span className="text-xs text-white/40">{vault.linked_wallets} linked</span>
      </div>

      {!isAuthority ? (
        <p className="text-xs text-white/40">Only the vault authority can manage linked wallets.</p>
      ) : (
        <div className="space-y-3">
          {/* Link new wallet */}
          <div>
            <label className="text-xs text-white/50 mb-1 block">Link Wallet</label>
            <div className="flex gap-2">
              <input
                type="text"
                value={walletAddress}
                onChange={(e) => setWalletAddress(e.target.value)}
                placeholder="Wallet address..."
                className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-xs text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors min-w-0"
              />
              <button
                onClick={handleLink}
                disabled={loading || !walletAddress.trim()}
                className="px-3 py-2 text-xs font-medium rounded-lg bg-accent/20 text-accent hover:bg-accent/30 transition-colors cursor-pointer disabled:opacity-50 whitespace-nowrap"
              >
                {loading ? '...' : 'Link'}
              </button>
            </div>
          </div>

          {/* Check/Unlink wallet */}
          <div>
            <label className="text-xs text-white/50 mb-1 block">Check / Unlink</label>
            <div className="flex gap-2">
              <input
                type="text"
                value={checkAddress}
                onChange={(e) => { setCheckAddress(e.target.value); setCheckResult(null) }}
                placeholder="Check if wallet is linked..."
                className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-xs text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors min-w-0"
              />
              <button
                onClick={handleCheckAndUnlink}
                disabled={checking || !checkAddress.trim()}
                className="px-3 py-2 text-xs font-medium rounded-lg bg-white/10 text-white/70 hover:bg-white/20 transition-colors cursor-pointer disabled:opacity-50 whitespace-nowrap"
              >
                {checking ? '...' : 'Check'}
              </button>
            </div>
            {checkResult && (
              <div className="mt-2 flex items-center justify-between">
                <span className="text-xs text-white/50">
                  {shortenAddress(checkResult.wallet)}:{' '}
                  {checkResult.linked ? (
                    <span className="text-success">Linked</span>
                  ) : (
                    <span className="text-white/30">Not linked</span>
                  )}
                </span>
                {checkResult.linked && (
                  <button
                    onClick={() => handleUnlink(checkResult.wallet)}
                    disabled={!!unlinkLoading}
                    className="px-2 py-1 text-xs rounded bg-danger/20 text-danger hover:bg-danger/30 transition-colors cursor-pointer disabled:opacity-50"
                  >
                    {unlinkLoading === checkResult.wallet ? '...' : 'Unlink'}
                  </button>
                )}
              </div>
            )}
          </div>

          {error && <p className="text-xs text-danger">{error}</p>}
          {success && <p className="text-xs text-success">{success}</p>}
        </div>
      )}
    </div>
  )
}
