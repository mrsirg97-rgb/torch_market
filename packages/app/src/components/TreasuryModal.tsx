'use client'

import { useCallback, useEffect, useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { Transaction, TransactionInstruction } from '@solana/web3.js'
import {
  getProtocolTreasuryPda,
  getProtocolTreasuryState,
  type ProtocolTreasuryInfo,
} from 'torchsdk'
import {
  formatSol,
  formatSolAmount,
  shortenAddress,
  EPOCH_DURATION_SECONDS,
  PROGRAM_ID,
} from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'

interface TreasuryModalProps {
  isOpen: boolean
  onClose: () => void
}

export function TreasuryModal({ isOpen, onClose }: TreasuryModalProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()
  const [treasury, setTreasury] = useState<ProtocolTreasuryInfo | null>(null)
  const [onChainLamports, setOnChainLamports] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const [currentTime, setCurrentTime] = useState(Math.floor(Date.now() / 1000))
  const [cranking, setCranking] = useState(false)
  const [crankSuccess, setCrankSuccess] = useState<string | null>(null)
  const [crankError, setCrankError] = useState<string | null>(null)

  const treasuryAddress = getProtocolTreasuryPda()[0].toString()

  // Lock body scroll when modal is open
  useEffect(() => {
    if (isOpen) {
      document.body.style.overflow = 'hidden'
    } else {
      document.body.style.overflow = ''
    }
    return () => {
      document.body.style.overflow = ''
    }
  }, [isOpen])

  const fetchTreasury = useCallback(async () => {
    try {
      const [treasuryPda] = getProtocolTreasuryPda()
      const [state, accountInfo] = await Promise.all([
        getProtocolTreasuryState(connection),
        connection.getAccountInfo(treasuryPda),
      ])

      setTreasury(state)
      setOnChainLamports(accountInfo ? accountInfo.lamports : null)
    } catch (error) {
      if (isDev) console.error('Error fetching protocol treasury:', error)
      setTreasury(null)
    } finally {
      setLoading(false)
    }
  }, [connection])

  useEffect(() => {
    if (!isOpen) return

    fetchTreasury()

    // Update current time every second for countdown
    const interval = setInterval(() => {
      setCurrentTime(Math.floor(Date.now() / 1000))
    }, 1000)

    return () => clearInterval(interval)
  }, [connection, isOpen, fetchTreasury])

  async function handleCrank() {
    if (!wallet.publicKey) return

    setCranking(true)
    setCrankSuccess(null)
    setCrankError(null)

    try {
      const [treasuryPda] = getProtocolTreasuryPda()

      // Build the advance_protocol_epoch instruction from the IDL discriminator
      const discriminator = Buffer.from([215, 39, 184, 104, 13, 104, 63, 21])
      const ix = new TransactionInstruction({
        programId: PROGRAM_ID,
        keys: [
          { pubkey: wallet.publicKey, isSigner: true, isWritable: true },
          { pubkey: treasuryPda, isSigner: false, isWritable: true },
        ],
        data: discriminator,
      })

      const tx = new Transaction().add(ix)
      tx.feePayer = wallet.publicKey
      tx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash

      const txId = await sendTransaction(tx)
      await connection.confirmTransaction(txId, 'confirmed')

      setCrankSuccess('Epoch advanced successfully')
      await fetchTreasury()
      setTimeout(() => setCrankSuccess(null), 3000)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Transaction failed'
      if (msg.includes('User rejected')) {
        setCrankError('Transaction cancelled')
      } else {
        setCrankError(msg.length > 100 ? 'Failed to advance epoch. Please try again.' : msg)
      }
      setTimeout(() => setCrankError(null), 5000)
    } finally {
      setCranking(false)
    }
  }


  if (!isOpen) return null

  // Calculate epoch progress
  const lastEpochTs = treasury?.last_epoch_ts ?? 0
  const nextEpochTs = lastEpochTs + EPOCH_DURATION_SECONDS
  const timeUntilNextEpoch = Math.max(0, nextEpochTs - currentTime)
  const epochProgress =
    lastEpochTs > 0
      ? Math.min(100, ((currentTime - lastEpochTs) / EPOCH_DURATION_SECONDS) * 100)
      : 0

  // Format time remaining
  const days = Math.floor(timeUntilNextEpoch / (24 * 60 * 60))
  const hours = Math.floor((timeUntilNextEpoch % (24 * 60 * 60)) / (60 * 60))
  const minutes = Math.floor((timeUntilNextEpoch % (60 * 60)) / 60)
  const seconds = timeUntilNextEpoch % 60

  const formatTimeRemaining = () => {
    if (days > 0) return `${days}d ${hours}h ${minutes}m`
    if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`
    if (minutes > 0) return `${minutes}m ${seconds}s`
    return `${seconds}s`
  }

  // Use on-chain distributable_amount directly (reserve_floor is now 0)
  const distributableAmountSol = treasury?.distributable_amount_sol ?? 0

  return (
    <div
      className="fixed inset-0 modal-backdrop z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="modal-surface p-6 max-w-md w-full max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-6">
          <h2
            className="text-xl font-bold lowercase flex items-center gap-2"
            style={{ color: 'var(--foreground)' }}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="text-accent"
            >
              <rect x="3" y="3" width="18" height="18" rx="2" />
              <circle cx="12" cy="12" r="3" />
              <path d="M12 3v3" />
              <path d="M12 18v3" />
              <path d="M3 12h3" />
              <path d="M18 12h3" />
            </svg>
            protocol treasury
          </h2>
          <a
            href={`https://solscan.io/account/${treasuryAddress}`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-xs font-mono transition-colors hover:text-accent"
            style={{ color: 'var(--muted)' }}
            title="View on Solscan"
          >
            {shortenAddress(treasuryAddress)}
          </a>
          <button
            onClick={onClose}
            className="text-2xl cursor-pointer"
            style={{ color: 'var(--muted)' }}
          >
            &times;
          </button>
        </div>

        {/* Scrollable Content */}
        <div className="flex-1 overflow-y-auto min-h-0 overscroll-contain">
          {loading ? (
            <div className="text-center py-8">
              <p className="text-white/50">Loading treasury data...</p>
            </div>
          ) : !treasury ? (
            <div className="text-center py-8">
              <p className="text-white/50 mb-2">Treasury not initialized</p>
              <p className="text-white/30 text-sm">The protocol treasury will be activated soon.</p>
            </div>
          ) : (
            <div className="space-y-6">
              {/* Treasury Balance */}
              <div className="bg-white/5 rounded-lg p-4">
                <p className="text-white/50 text-sm mb-1">On-chain Balance</p>
                <p className="text-3xl font-bold text-accent">
                  {onChainLamports !== null
                    ? formatSol(onChainLamports)
                    : formatSolAmount(treasury.current_balance_sol)}{' '}
                  SOL
                </p>
                <div className="flex items-center justify-end mt-2">
                  <p className="text-white/60 text-xs">
                    Distributable: {formatSolAmount(distributableAmountSol)} SOL
                  </p>
                </div>
              </div>


              {/* Epoch Progress */}
              <div>
                <div className="flex items-center justify-between mb-2">
                  <p className="text-white/70 text-sm">Epoch {treasury.current_epoch}</p>
                  <p className="text-white/50 text-sm">
                    {lastEpochTs > 0 ? formatTimeRemaining() : 'Not started'}
                  </p>
                </div>
                <div className="h-3 bg-white/10 rounded-full overflow-hidden">
                  <div
                    className="h-full bg-gradient-to-r from-accent to-accent/70 transition-all duration-1000"
                    style={{ width: `${epochProgress}%` }}
                  />
                </div>
                <p className="text-white/40 text-xs mt-2">
                  Rewards distributed weekly to traders with 2+ SOL epoch volume
                </p>
                {timeUntilNextEpoch === 0 && (
                  <div className="mt-3">
                    <button
                      onClick={handleCrank}
                      disabled={cranking || !wallet.publicKey}
                      className="w-full py-2 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/10 text-white hover:bg-white/20"
                    >
                      {cranking ? 'Advancing...' : 'Advance Epoch'}
                    </button>
                    {crankSuccess && <p className="text-xs text-success mt-2">{crankSuccess}</p>}
                    {crankError && <p className="text-xs text-danger mt-2">{crankError}</p>}
                  </div>
                )}
              </div>

              {/* Stats Grid */}
              <div className="grid grid-cols-2 gap-3">
                <div className="bg-white/5 rounded-lg p-3 text-center">
                  <p className="text-white/50 text-xs mb-1">Total Fees Received</p>
                  <p className="text-lg font-bold">
                    {formatSolAmount(treasury.total_fees_received_sol)} SOL
                  </p>
                </div>
                <div className="bg-white/5 rounded-lg p-3 text-center">
                  <p className="text-white/50 text-xs mb-1">Total Distributed</p>
                  <p className="text-lg font-bold">
                    {formatSolAmount(treasury.total_distributed_sol)} SOL
                  </p>
                </div>
                <div className="bg-white/5 rounded-lg p-3 text-center">
                  <p className="text-white/50 text-xs mb-1">Epoch Volume</p>
                  <p className="text-lg font-bold">
                    {formatSolAmount(treasury.total_volume_current_epoch_sol)} SOL
                  </p>
                </div>
                <div className="bg-white/5 rounded-lg p-3 text-center">
                  <p className="text-white/50 text-xs mb-1">Prev Epoch Volume</p>
                  <p className="text-lg font-bold">
                    {formatSolAmount(treasury.total_volume_previous_epoch_sol)} SOL
                  </p>
                </div>
              </div>

              {/* Info */}
              <div className="bg-accent/10 border border-accent/20 rounded-lg p-3">
                <p className="text-accent text-sm font-medium mb-1">How it works</p>
                <p className="text-white/60 text-xs">
                  0.5% protocol fee from bonding phase trades accumulates here. Each week, all
                  accumulated SOL is distributed to traders based on their volume share.
                  Trade 2+ SOL per epoch to qualify.
                </p>
              </div>

              {/* Open Source Note */}
              <div className="border-t border-white/10 pt-4 mt-2">
                <p className="text-white/40 text-xs text-center">
                  All protocol fees are distributed to active traders. No reserve floor — 100% of accumulated SOL goes to epoch rewards.
                </p>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
