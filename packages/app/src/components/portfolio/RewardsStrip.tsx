'use client'

import { useState, useCallback, useEffect } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import {
  buildClaimProtocolRewardsTransaction,
  getUserStats,
  getProtocolTreasuryState,
} from 'torchsdk'
import { LAMPORTS_PER_SOL, MIN_EPOCH_VOLUME_ELIGIBILITY } from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'
const MIN_EPOCH_VOLUME_ELIGIBILITY_SOL = Number(MIN_EPOCH_VOLUME_ELIGIBILITY) / LAMPORTS_PER_SOL

/**
 * Protocol rewards balance + claim button.
 * Wallet must be connected — render nothing otherwise.
 */
export function RewardsStrip() {
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const sendTransaction = useMwaSendTransaction()
  const [claimableSol, setClaimableSol] = useState(0)
  const [loading, setLoading] = useState(false)
  const [claiming, setClaiming] = useState(false)
  const [result, setResult] = useState<{ type: 'success' | 'error'; message: string } | null>(null)

  const fetchRewards = useCallback(async () => {
    if (!publicKey) {
      setClaimableSol(0)
      return
    }
    setLoading(true)
    try {
      const [userStats, treasury] = await Promise.all([
        getUserStats(connection, publicKey.toString()),
        getProtocolTreasuryState(connection),
      ])
      if (!treasury || !userStats) {
        setClaimableSol(0)
        return
      }

      const currentEpoch = treasury.current_epoch
      const distributableSol = treasury.distributable_amount_sol
      const totalVolPrevSol = treasury.total_volume_previous_epoch_sol
      let userVolPrevSol = userStats.volume_previous_epoch_sol
      const userVolCurrentSol = userStats.volume_current_epoch_sol
      const lastVolumeEpoch = userStats.last_volume_epoch
      const lastEpochClaimed = userStats.last_epoch_claimed

      if (lastVolumeEpoch < currentEpoch && userVolCurrentSol > 0) {
        userVolPrevSol = userVolCurrentSol
      }

      const claimableEpoch = currentEpoch - 1
      if (
        currentEpoch === 0 ||
        lastEpochClaimed >= claimableEpoch ||
        distributableSol === 0 ||
        userVolPrevSol === 0 ||
        userVolPrevSol < MIN_EPOCH_VOLUME_ELIGIBILITY_SOL ||
        totalVolPrevSol === 0
      ) {
        setClaimableSol(0)
        return
      }

      const userShareSol = (userVolPrevSol / totalVolPrevSol) * distributableSol
      setClaimableSol(Math.max(0, userShareSol))
    } catch (err) {
      if (isDev) console.error('RewardsStrip: fetch failed', err)
      setClaimableSol(0)
    } finally {
      setLoading(false)
    }
  }, [publicKey, connection])

  useEffect(() => {
    fetchRewards()
  }, [fetchRewards])

  const handleClaim = useCallback(async () => {
    if (!publicKey || claimableSol <= 0) return
    setClaiming(true)
    setResult(null)
    try {
      const { transaction } = await buildClaimProtocolRewardsTransaction(connection, {
        user: publicKey.toString(),
      })
      const txId = await sendTransaction(transaction)
      const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash()
      await connection.confirmTransaction(
        { signature: txId, blockhash, lastValidBlockHeight },
        'confirmed',
      )
      setResult({ type: 'success', message: `claimed ${claimableSol.toFixed(4)} SOL` })
      setClaimableSol(0)
      setTimeout(() => {
        fetchRewards()
        setResult(null)
      }, 5000)
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'claim failed'
      setResult({ type: 'error', message: msg })
      setTimeout(() => setResult(null), 5000)
    } finally {
      setClaiming(false)
    }
  }, [publicKey, sendTransaction, claimableSol, fetchRewards, connection])

  if (!publicKey) return null

  return (
    <div className="card p-4 mb-8 flex items-center justify-between">
      <div>
        <p className="text-xs lowercase tracking-wide" style={{ color: 'var(--muted)' }}>
          protocol rewards
        </p>
        <p className="text-lg font-mono font-bold mt-0.5" style={{ color: 'var(--foreground)' }}>
          {loading ? (
            <span style={{ color: 'var(--muted)' }}>…</span>
          ) : (
            <>{claimableSol.toFixed(4)} SOL</>
          )}
        </p>
        {result && (
          <p className={`text-xs mt-1 ${result.type === 'success' ? 'text-success' : 'text-danger'}`}>
            {result.message}
          </p>
        )}
      </div>
      <button
        onClick={handleClaim}
        disabled={claiming || claimableSol <= 0 || loading}
        className="btn btn-accent text-sm cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
      >
        {claiming ? 'claiming…' : 'claim'}
      </button>
    </div>
  )
}
