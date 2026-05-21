'use client'

import { useWallet } from '@solana/wallet-adapter-react'
import type { VaultInfo } from 'torchsdk'
import { shortenAddress } from '@/lib/constants'

interface VaultDashboardProps {
  vault: VaultInfo
}

function fmtSol(val: number): string {
  return val.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 })
}

export function VaultDashboard({ vault }: VaultDashboardProps) {
  const { publicKey } = useWallet()

  const isAuthority = publicKey?.toString() === vault.authority
  const isCreator = publicKey?.toString() === vault.creator

  return (
    <div className="card border border-white/10 rounded-xl p-4">
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-semibold text-white/80">Vault Overview</h3>
        <div className="flex items-center gap-2">
          {isAuthority && (
            <span className="text-xs bg-accent/20 text-accent px-2 py-0.5 rounded-full">Authority</span>
          )}
          {isCreator && !isAuthority && (
            <span className="text-xs bg-purple-500/20 text-purple-400 px-2 py-0.5 rounded-full">Creator</span>
          )}
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">SOL Balance</p>
          <p className="text-lg font-semibold text-white">{fmtSol(vault.sol_balance)}</p>
          <p className="text-xs text-white/30">SOL</p>
        </div>
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">Linked Wallets</p>
          <p className="text-lg font-semibold text-white">{vault.linked_wallets}</p>
          <p className="text-xs text-white/30">wallets</p>
        </div>
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">Total Deposited</p>
          <p className="text-sm font-medium text-white">{fmtSol(vault.total_deposited)}</p>
        </div>
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">Total Withdrawn</p>
          <p className="text-sm font-medium text-white">{fmtSol(vault.total_withdrawn)}</p>
        </div>
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">Total Spent</p>
          <p className="text-sm font-medium text-white">{fmtSol(vault.total_spent)}</p>
        </div>
        <div className="bg-white/5 rounded-lg p-3">
          <p className="text-xs text-white/40 mb-1">Total Received</p>
          <p className="text-sm font-medium text-white">{fmtSol(vault.total_received)}</p>
        </div>
      </div>

      <div className="mt-3 pt-3 border-t border-white/5">
        <div className="flex items-center justify-between text-xs text-white/40">
          <span>Vault Address</span>
          <span className="font-mono">{shortenAddress(vault.address, 6)}</span>
        </div>
        {vault.authority !== vault.creator && (
          <div className="flex items-center justify-between text-xs text-white/40 mt-1">
            <span>Authority</span>
            <span className="font-mono">{shortenAddress(vault.authority, 6)}</span>
          </div>
        )}
      </div>
    </div>
  )
}
