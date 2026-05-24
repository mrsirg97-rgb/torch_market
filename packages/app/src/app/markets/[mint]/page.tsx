'use client'

import { useState, useEffect, useCallback, use } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token'
import Link from 'next/link'
import { buildStarTransaction, buildSwapFeesToSolTransaction, getVault } from 'torchsdk'
import { Header, TreasuryModal, HowItWorksModal } from '@/components'
import { useSaidVerificationBatch } from '@/hooks/useSaidVerification'
import {
  MarketViewPanel,
  MarginPanel,
  SwapPanel,
  UserPosition,
  type SwapPreview,
} from '@/components/token'
import { useToken } from '@/hooks/useToken'
import { isLegacyMint } from '@/types/token'
import { formatSol, formatTokens } from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'

export default function TokenPage({ params }: { params: Promise<{ mint: string }> }) {
  const resolvedParams = use(params)
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  // Outer action-panel toggle: Trade (spot) or Margin (short/borrow)
  const [actionMode, setActionMode] = useState<'trade' | 'margin'>('trade')

  // Panel state
  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [showTreasuryModal, setShowTreasuryModal] = useState(false)

  // Star error/success
  const [starError, setStarError] = useState<string | null>(null)
  const [starSuccess, setStarSuccess] = useState<string | null>(null)

  // Treasury crank state
  const [crankLoading, setCrankLoading] = useState<'harvest' | null>(null)
  const [crankMsg, setCrankMsg] = useState<string | null>(null)

  // Copy state
  const [copiedMint, setCopiedMint] = useState(false)
  const [copiedCreator, setCopiedCreator] = useState(false)

  // Holders count
  const [holdersCount, setHoldersCount] = useState<number | null>(null)

  // Swap preview state (passed up from SwapPanel)
  const [swapPreview, setSwapPreview] = useState<SwapPreview>(null)

  // Token data
  const token = useToken(resolvedParams.mint)

  // Vault token balance for current mint
  const [vaultTokenBalance, setVaultTokenBalance] = useState<bigint | null>(null)

  const fetchVaultTokenBalance = useCallback(async () => {
    if (!wallet.publicKey || !token.mint) {
      setVaultTokenBalance(null)
      return
    }
    try {
      const vault = await getVault(connection, wallet.publicKey.toString())
      if (!vault) {
        setVaultTokenBalance(null)
        return
      }
      const vaultPda = new PublicKey(vault.address)
      const vaultAta = getAssociatedTokenAddressSync(
        token.mint,
        vaultPda,
        true,
        TOKEN_2022_PROGRAM_ID,
      )
      const ataAccount = await connection.getAccountInfo(vaultAta)
      if (ataAccount && ataAccount.data.length >= 72) {
        setVaultTokenBalance(ataAccount.data.readBigUInt64LE(64))
      } else {
        setVaultTokenBalance(BigInt(0))
      }
    } catch {
      setVaultTokenBalance(null)
    }
  }, [connection, wallet.publicKey, token.mint])

  useEffect(() => {
    fetchVaultTokenBalance()
  }, [fetchVaultTokenBalance])

  // SAID verification for message senders
  const messageSenders = token.messages?.map((m) => m.sender) ?? []
  const saidVerifications = useSaidVerificationBatch(messageSenders)

  // Fetch holders count (deferred, server-side cached)
  useEffect(() => {
    if (!token.mint) return
    const timeout = setTimeout(() => {
      fetch(`/api/holders-count/${token.mint!.toString()}`)
        .then((res) => res.json())
        .then((data) => {
          if (data?.holders != null) {
            setHoldersCount(data.holders)
          }
        })
        .catch(() => {})
    }, 4000)
    return () => clearTimeout(timeout)
  }, [token.mint])

  // Star handler
  async function handleStarToken() {
    if (!wallet.publicKey || !token.tokenDetail || !token.mint) return

    if (wallet.publicKey.toString() === token.creator) {
      setStarError('You cannot star your own token')
      return
    }

    token.setStarLoading(true)
    setStarError(null)

    try {
      const { transaction } = await buildStarTransaction(connection, {
        mint: token.mintAddress,
        user: wallet.publicKey.toString(),
      })

      const latestBlockhash = await connection.getLatestBlockhash()
      const signature = await sendTransaction(transaction)

      try {
        const confirmation = await connection.confirmTransaction(
          {
            signature,
            blockhash: latestBlockhash.blockhash,
            lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,
          },
          'confirmed',
        )
        if (confirmation.value.err) {
          throw new Error('Transaction failed on-chain')
        }
      } catch (confirmErr) {
        const msg = confirmErr instanceof Error ? confirmErr.message : ''
        if (
          !msg.includes('block height exceeded') &&
          !msg.includes('TransactionExpiredBlockheightExceededError')
        ) {
          throw confirmErr
        }
        if (isDev)
          console.warn('Confirmation timed out, but transaction may have succeeded:', signature)
      }

      await token.fetchStarRecord()
      await token.fetchToken()
      token.setHasStarred(true)
      setStarSuccess('Star recorded! (0.02 SOL)')
      setTimeout(() => setStarSuccess(null), 3000)
    } catch (err: unknown) {
      if (isDev) console.error('Error starring token:', err)
      const errorMessage = err instanceof Error ? err.message : 'Failed to star token'
      if (errorMessage.includes('already in use') || errorMessage.includes('AlreadyStarred')) {
        setStarError('You have already starred this token')
      } else {
        setStarError(errorMessage)
      }
    } finally {
      token.setStarLoading(false)
    }
  }

  function handleTradeComplete() {
    token.fetchMessages()
    token.fetchToken()
    fetchVaultTokenBalance()
  }

  const handleCopyMint = () => {
    navigator.clipboard.writeText(token.mintAddress)
    setCopiedMint(true)
    setTimeout(() => setCopiedMint(false), 1500)
  }

  const handleCopyCreator = () => {
    if (token.creator) {
      navigator.clipboard.writeText(token.creator)
      setCopiedCreator(true)
      setTimeout(() => setCopiedCreator(false), 1500)
    }
  }

  // Permissionless treasury crank — harvest withheld fees + swap to SOL.
  async function handleHarvestCrank() {
    if (!wallet.publicKey) return
    setCrankLoading('harvest')
    setCrankMsg(null)
    try {
      const result = await buildSwapFeesToSolTransaction(connection, {
        mint: token.mintAddress,
        payer: wallet.publicKey.toString(),
      })
      const allTxs = [result.transaction, ...(result.additionalTransactions || [])]
      for (const tx of allTxs) {
        await sendTransaction(tx)
      }
      setCrankMsg(
        allTxs.length > 1 ? 'Fees harvested & swapped (2 txs)' : 'Fees harvested & swapped to SOL',
      )
      setTimeout(() => {
        setCrankMsg(null)
        token.fetchToken()
      }, 3000)
    } catch (e) {
      const msg = (e as Error)?.message || 'Failed'
      if (msg.includes('User rejected')) setCrankMsg('Cancelled')
      else if (
        msg.includes('NoFeesToSwap') ||
        msg.includes('insufficient') ||
        msg.includes('No fees')
      )
        setCrankMsg('No fees to swap')
      else if (msg.includes('Price below threshold') || msg.includes('below sell'))
        setCrankMsg('Price below sell threshold')
      else if (msg.includes('cooldown') || msg.includes('Cooldown'))
        setCrankMsg('Cooldown not elapsed')
      else setCrankMsg('Error: ' + msg.slice(0, 80))
    } finally {
      setCrankLoading(null)
    }
  }

  // ─── Early states ───────────────────────────────────────────────────
  if (!token.isValidMint) {
    return (
      <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
        <Header
          onHowItWorksClick={() => setShowHowItWorks(true)}
          onTreasuryClick={() => setShowTreasuryModal(true)}
        />
        <main className="max-w-3xl mx-auto px-4 py-16 w-full text-center">
          <p className="text-danger mb-2">invalid market address</p>
          <p className="text-sm mb-6" style={{ color: 'var(--muted)' }}>
            the provided address is not a valid Solana address.
          </p>
          <Link href="/markets" className="btn btn-accent">
            browse markets
          </Link>
        </main>
        <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
        <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
      </div>
    )
  }

  if (token.loading) {
    return (
      <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
        <Header
          onHowItWorksClick={() => setShowHowItWorks(true)}
          onTreasuryClick={() => setShowTreasuryModal(true)}
        />
        <main className="max-w-3xl mx-auto px-4 py-16 w-full text-center">
          <p style={{ color: 'var(--muted)' }}>loading market…</p>
        </main>
        <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
        <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
      </div>
    )
  }

  if (!token.tokenDetail || !token.mint) {
    return (
      <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
        <Header
          onHowItWorksClick={() => setShowHowItWorks(true)}
          onTreasuryClick={() => setShowTreasuryModal(true)}
        />
        <main className="max-w-3xl mx-auto px-4 py-16 w-full text-center">
          <p className="mb-6" style={{ color: 'var(--muted)' }}>
            market not found
          </p>
          <Link href="/markets" className="btn btn-accent">
            browse markets
          </Link>
        </main>
        <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
        <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
      </div>
    )
  }

  const isOwnToken = wallet.publicKey?.toString() === token.creator

  // ─── Main render ────────────────────────────────────────────────────
  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header
        onHowItWorksClick={() => setShowHowItWorks(true)}
        onTreasuryClick={() => setShowTreasuryModal(true)}
      />

      <main className="w-full">
        <div className="w-full max-w-screen-2xl mx-auto px-4 sm:px-6 lg:px-8 py-2">
          {/* Breadcrumb + star messages */}
          <div className="flex items-center justify-between mb-2 gap-2">
            <Link
              href="/markets"
              className="text-sm transition-colors"
              style={{ color: 'var(--muted)' }}
            >
              ← markets
            </Link>
            <div className="flex items-center gap-2">
              {starError && (
                <div
                  className="rounded-full px-3 py-1 text-danger text-xs"
                  style={{ background: 'color-mix(in srgb, var(--danger) 14%, transparent)' }}
                >
                  {starError}
                </div>
              )}
              {starSuccess && (
                <div
                  className="rounded-full px-3 py-1 text-success text-xs"
                  style={{ background: 'color-mix(in srgb, var(--success) 14%, transparent)' }}
                >
                  {starSuccess}
                </div>
              )}
            </div>
          </div>

          {/* Two-column main layout: MarketViewPanel left, action area right */}
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
            {/* Left 2/3 — chart + info */}
            <div className="lg:col-span-2">
              <MarketViewPanel
                token={token}
                mint={token.mint}
                holdersCount={holdersCount}
                messages={token.messages}
                saidVerifications={saidVerifications}
                copiedMint={copiedMint}
                onCopyMint={handleCopyMint}
                onCopyCreator={handleCopyCreator}
                copiedDev={copiedCreator}
                onStarClick={handleStarToken}
                walletConnected={!!wallet.publicKey}
                isOwnToken={isOwnToken}
                crankMsg={crankMsg}
                crankLoading={crankLoading}
                onHarvestCrank={handleHarvestCrank}
              />
            </div>

            {/* Right 1/3 — action panel + (when Trade) user position */}
            <div className="flex flex-col gap-3">
              {/* Trade / Margin outer toggle */}
              <div className="flex gap-1 rounded-lg bg-white/5 p-0.5">
                <button
                  onClick={() => setActionMode('trade')}
                  className={`flex-1 py-1.5 text-sm font-medium rounded-md transition-colors cursor-pointer ${
                    actionMode === 'trade'
                      ? 'bg-[color-mix(in_srgb,var(--accent)_22%,transparent)] text-[var(--accent)]'
                      : 'text-white/50 hover:text-white/70'
                  }`}
                >
                  Trade
                </button>
                <button
                  onClick={() => setActionMode('margin')}
                  className={`flex-1 py-1.5 text-sm font-medium rounded-md transition-colors cursor-pointer ${
                    actionMode === 'margin'
                      ? 'bg-[color-mix(in_srgb,var(--accent)_22%,transparent)] text-[var(--accent)]'
                      : 'text-white/50 hover:text-white/70'
                  }`}
                >
                  Margin
                </button>
              </div>

              {actionMode === 'trade' ? (
                <>
                  <div className="card">
                    <SwapPanel
                      mintAddress={token.mintAddress}
                      userTokenBalance={token.userTokenBalance}
                      vaultTokenBalance={vaultTokenBalance}
                      symbol={token.symbol}
                      isMigrated={token.isMigrated}
                      isComplete={token.isComplete}
                      isVoting={token.isVoting}
                      isLegacy={isLegacyMint(token.mintAddress)}
                      onTradeComplete={handleTradeComplete}
                      onPreviewChange={setSwapPreview}
                    />
                  </div>
                  {/* Spot user position — only shown in Trade mode */}
                  {wallet.publicKey && (
                    <div className="card p-3">
                      {swapPreview && (
                        <div className="bg-white/5 rounded-lg p-3 text-xs mb-3">
                          <h4 className="text-white/50 mb-2">Trade Preview</h4>
                          {swapPreview.type === 'buy' ? (
                            <div className="space-y-1">
                              <Row label="You receive" value={`${formatTokens(swapPreview.tokensToUser)} ${swapPreview.symbol}`} />
                              <Row label="Min. guaranteed" value={`${formatTokens(swapPreview.minGuaranteed)} ${swapPreview.symbol}`} valueColor="var(--success)" />
                              <Row label="To treasury" value={`${formatTokens(swapPreview.tokensToCommunity)} ${swapPreview.symbol}`} valueColor="var(--accent)" />
                              <Row label="Protocol fee" value={`${formatSol(swapPreview.protocolFee)} SOL`} />
                            </div>
                          ) : (
                            <div className="space-y-1">
                              <Row label="You receive" value={`${formatSol(swapPreview.solToUser)} SOL`} />
                              <Row label="Min. guaranteed" value={`${formatSol(swapPreview.minGuaranteed)} SOL`} valueColor="var(--success)" />
                            </div>
                          )}
                        </div>
                      )}
                      <UserPosition
                        userTokenBalance={token.userTokenBalance}
                        vaultTokenBalance={vaultTokenBalance}
                        symbol={token.symbol}
                        priceHistory={token.priceHistory}
                        walletAddress={wallet.publicKey.toBase58()}
                      />
                    </div>
                  )}
                </>
              ) : (
                <MarginPanel
                  mintAddress={token.mintAddress}
                  isMigrated={token.isMigrated}
                  symbol={token.symbol}
                  userTokenBalance={token.userTokenBalance}
                  vaultTokenBalance={vaultTokenBalance}
                  priceInSol={token.priceInSol}
                  treasurySolBalance={token.treasurySolBalance}
                  utilizationCapBps={token.utilizationCapBps}
                  totalSolLent={token.totalSolLent}
                />
              )}
            </div>
          </div>
        </div>
      </main>

      <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />
      <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
    </div>
  )
}

// Trade-preview helpers (inline — pulled from old TradingTab to keep the
// preview rendering local to this page).
function Row({
  label,
  value,
  valueColor,
}: {
  label: string
  value: string
  valueColor?: string
}) {
  return (
    <div className="flex justify-between">
      <span className="text-white/50">{label}:</span>
      <span style={{ color: valueColor ?? 'var(--foreground)' }}>{value}</span>
    </div>
  )
}

