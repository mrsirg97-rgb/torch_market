'use client'

import { useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { buildDepositVaultTransaction, buildWithdrawVaultTransaction } from 'torchsdk'
import type { VaultInfo } from 'torchsdk'
import { LAMPORTS_PER_SOL } from '@/lib/constants'

interface VaultActionsProps {
  vault: VaultInfo
  onSuccess: () => void
}

export function VaultActions({ vault, onSuccess }: VaultActionsProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  const [tab, setTab] = useState<'deposit' | 'withdraw'>('deposit')
  const [amount, setAmount] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const isAuthority = wallet.publicKey?.toString() === vault.authority

  async function handleAction() {
    if (!wallet.publicKey) return

    const solAmount = parseFloat(amount)
    if (isNaN(solAmount) || solAmount <= 0) {
      setError('Enter a valid SOL amount')
      return
    }

    setLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const lamports = Math.floor(solAmount * LAMPORTS_PER_SOL)

      let tx
      if (tab === 'deposit') {
        const result = await buildDepositVaultTransaction(connection, {
          depositor: wallet.publicKey.toString(),
          vault_creator: vault.creator,
          amount_sol: lamports,
        })
        tx = result.transaction
      } else {
        if (!isAuthority) {
          setError('Only the vault authority can withdraw')
          setLoading(false)
          return
        }
        const result = await buildWithdrawVaultTransaction(connection, {
          authority: wallet.publicKey.toString(),
          vault_creator: vault.creator,
          amount_sol: lamports,
        })
        tx = result.transaction
      }

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setAmount('')
      setSuccess(`${tab === 'deposit' ? 'Deposited' : 'Withdrew'} ${solAmount} SOL`)
      setTimeout(onSuccess, 1500)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Transaction failed'
      if (msg.includes('User rejected')) {
        setError('Transaction cancelled')
      } else {
        setError(msg.length > 100 ? 'Transaction failed. Please try again.' : msg)
      }
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="card border border-white/10 rounded-xl p-4">
      <h3 className="text-sm font-semibold text-white/80 mb-3">SOL Management</h3>

      <div className="flex mb-3 border-b border-white/10">
        <button
          onClick={() => { setTab('deposit'); setError(null); setSuccess(null) }}
          className={`flex-1 pb-2 text-sm text-center transition-colors cursor-pointer ${
            tab === 'deposit' ? 'text-success border-b-2 border-success' : 'text-white/50 hover:text-white/70'
          }`}
        >
          Deposit
        </button>
        <button
          onClick={() => { setTab('withdraw'); setError(null); setSuccess(null) }}
          className={`flex-1 pb-2 text-sm text-center transition-colors cursor-pointer ${
            tab === 'withdraw' ? 'text-accent border-b-2 border-accent' : 'text-white/50 hover:text-white/70'
          }`}
        >
          Withdraw
        </button>
      </div>

      <div className="space-y-3">
        <div>
          <div className="flex items-center justify-between mb-1">
            <label className="text-xs text-white/50">Amount (SOL)</label>
            <span className="text-xs text-white/40">
              Vault: {vault.sol_balance.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 })} SOL
            </span>
          </div>
          <input
            type="number"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            placeholder="0.0"
            className="w-full bg-white/5 border border-white/10 rounded-lg px-3 py-2.5 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
          />
          {tab === 'withdraw' && (
            <div className="flex gap-2 mt-2">
              {[25, 50, 75, 100].map((pct) => (
                <button
                  key={pct}
                  onClick={() => {
                    const solVal = (vault.sol_balance * pct) / 100
                    setAmount(solVal.toString())
                  }}
                  className="flex-1 py-1 text-xs rounded-lg bg-white/10 text-white/70 hover:bg-white/20 transition-colors cursor-pointer"
                >
                  {pct === 100 ? 'Max' : `${pct}%`}
                </button>
              ))}
            </div>
          )}
        </div>

        {tab === 'withdraw' && !isAuthority && (
          <p className="text-xs text-warning">Only the vault authority can withdraw SOL.</p>
        )}

        {error && <p className="text-xs text-danger">{error}</p>}
        {success && <p className="text-xs text-success">{success}</p>}

        <button
          onClick={handleAction}
          disabled={loading || !wallet.publicKey || (tab === 'withdraw' && !isAuthority)}
          className="w-full py-2.5 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/10 text-white hover:bg-white/20"
        >
          {loading ? 'Processing...' : tab === 'deposit' ? 'Deposit SOL' : 'Withdraw SOL'}
        </button>
      </div>
    </div>
  )
}
