'use client'

import { useState, useEffect, useCallback, use } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token'
import Link from 'next/link'
import Image from 'next/image'
import { buildStarTransaction, buildSwapFeesToSolTransaction, getVault } from 'torchsdk'
import {
  Header,
  TreasuryModal,
  HowItWorksModal,
  VerifiedBadge,
} from '@/components'
import { useSaidVerificationBatch } from '@/hooks/useSaidVerification'
import { TradingTab, LendingDashboard, type SwapPreview } from '@/components/token'
import { useToken } from '@/hooks/useToken'
import {
  shortenAddress,
  LAMPORTS_PER_SOL,
} from '@/lib/constants'
import { isLegacyMint } from '@/types/token'

const isDev = process.env.NODE_ENV === 'development'

/** Format a human-readable number of tokens using abbreviations (B/M/K) */
function formatTokenAmount(value: number): string {
  if (value >= 1_000_000_000) return (value / 1_000_000_000).toFixed(2) + 'B'
  if (value >= 1_000_000) return (value / 1_000_000).toFixed(2) + 'M'
  if (value >= 1_000) return (value / 1_000).toFixed(2) + 'K'
  return value.toFixed(2)
}

export default function TokenPage({ params }: { params: Promise<{ mint: string }> }) {
  const resolvedParams = use(params)
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  // Tab state
  const [activeTab, setActiveTab] = useState<'trading' | 'margin'>('trading')

  // Panel state
  const [showHowItWorks, setShowHowItWorks] = useState(false)
  const [showTreasuryModal, setShowTreasuryModal] = useState(false)

  // Star error state (kept at page level for error handling)
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

  // Token data hooks
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

  // Handle starring a token using SDK's buildStarTransaction
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

      // Get blockhash for confirmation
      const latestBlockhash = await connection.getLatestBlockhash()

      // Send via wallet (uses Phantom's signAndSendTransaction for proper popup behavior)
      const signature = await sendTransaction(transaction)

      // Confirm with graceful timeout handling
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
        // If it's a timeout/expiration, the tx may still succeed - continue
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

      // Refresh star record and token data
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

  // Handle trade complete (refresh data)
  function handleTradeComplete() {
    token.fetchMessages()
    token.fetchToken()
    fetchVaultTokenBalance()
  }

  // Copy handlers
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

  const truncateMiddle = (str: string, startChars = 6, endChars = 4) => {
    if (str.length <= startChars + endChars) return str
    return `${str.slice(0, startChars)}...${str.slice(-endChars)}`
  }

  // Handle invalid mint address
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

  if (!token.tokenDetail) {
    return (
      <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
        <Header
          onHowItWorksClick={() => setShowHowItWorks(true)}
          onTreasuryClick={() => setShowTreasuryModal(true)}
        />
        <main className="max-w-3xl mx-auto px-4 py-16 w-full text-center">
          <p className="mb-6" style={{ color: 'var(--muted)' }}>market not found</p>
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
  const progress = token.progress

  return (
    <div className="min-h-mobile-screen" style={{ background: 'var(--background)' }}>
      <Header
        onHowItWorksClick={() => setShowHowItWorks(true)}
        onTreasuryClick={() => setShowTreasuryModal(true)}
      />

      <main className="w-full">
        <div className="w-full max-w-screen-2xl mx-auto px-4 sm:px-6 lg:px-8 py-2">
          <div className="flex items-center justify-between mb-2">
            <Link
              href="/markets"
              className="text-sm transition-colors"
              style={{ color: 'var(--muted)' }}
            >
              ← markets
            </Link>
            {/* Star error/success display */}
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

          {/* === HEADER ROW === */}
          <div className="card p-3 mb-3">
            <div className="flex flex-col lg:flex-row lg:items-start lg:justify-between gap-3">
              {/* Left: Token Image + Name/Ticker/Status + Addresses */}
              <div className="flex items-start gap-4">
                {/* Image */}
                {token.metadata?.image ? (
                  <Image
                    src={token.metadata.image}
                    alt={token.name}
                    width={96}
                    height={96}
                    className="w-24 h-24 rounded-xl object-cover flex-shrink-0"
                    unoptimized
                  />
                ) : (
                  <div className="w-24 h-24 rounded-xl bg-gradient-to-br from-accent/20 to-danger/20 flex items-center justify-center text-3xl flex-shrink-0">
                    {token.symbol.charAt(0)}
                  </div>
                )}
                {/* Info column */}
                <div>
                  {/* Name row */}
                  <div className="flex items-center gap-2">
                    <h1 className="text-2xl font-bold text-white">{token.name}</h1>
                  </div>
                  {/* Ticker + Status chip row under name */}
                  <div className="flex items-center gap-2 mt-1">
                    <p className="text-white/50 text-sm font-medium">${token.symbol}</p>
                    <span
                      className={`badge text-xs ${
                        token.isMigrated
                          ? 'badge-migrated'
                          : token.isComplete || token.isVoting
                            ? 'badge-complete'
                            : 'badge-bonding'
                      }`}
                    >
                      {token.isMigrated
                        ? 'Migrated'
                        : token.isComplete || token.isVoting
                          ? 'Complete'
                          : 'Bonding'}
                    </span>
                  </div>
                  {/* Addresses under ticker */}
                  <div className="mt-1 space-y-0.5">
                    {/* Contract Address - larger */}
                    <div className="flex items-center gap-2">
                      <button
                        onClick={handleCopyMint}
                        className="flex items-center gap-1 text-white/60 hover:text-white font-mono text-sm transition-colors group cursor-pointer"
                        title="Copy contract address"
                      >
                        <span>{truncateMiddle(token.mintAddress, 8, 6)}</span>
                        {copiedMint ? (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="12"
                            height="12"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            className="text-success"
                          >
                            <polyline points="20 6 9 17 4 12" />
                          </svg>
                        ) : (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="12"
                            height="12"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            className="opacity-50 group-hover:opacity-100"
                          >
                            <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                          </svg>
                        )}
                      </button>
                      {/* Social Links inline with CA */}
                      {(token.metadata?.twitter ||
                        token.metadata?.telegram ||
                        token.metadata?.website) && (
                        <div className="flex items-center gap-2">
                          {token.metadata.twitter && (
                            <a
                              href={token.metadata.twitter}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="text-white/30 hover:text-white transition-colors"
                              title="Twitter"
                            >
                              <svg
                                xmlns="http://www.w3.org/2000/svg"
                                width="12"
                                height="12"
                                viewBox="0 0 24 24"
                                fill="currentColor"
                              >
                                <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
                              </svg>
                            </a>
                          )}
                          {token.metadata.telegram && (
                            <a
                              href={token.metadata.telegram}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="text-white/30 hover:text-white transition-colors"
                              title="Telegram"
                            >
                              <svg
                                xmlns="http://www.w3.org/2000/svg"
                                width="12"
                                height="12"
                                viewBox="0 0 24 24"
                                fill="currentColor"
                              >
                                <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z" />
                              </svg>
                            </a>
                          )}
                          {token.metadata.website && (
                            <a
                              href={token.metadata.website}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="text-white/30 hover:text-white transition-colors"
                              title="Website"
                            >
                              <svg
                                xmlns="http://www.w3.org/2000/svg"
                                width="12"
                                height="12"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2"
                                strokeLinecap="round"
                                strokeLinejoin="round"
                              >
                                <circle cx="12" cy="12" r="10" />
                                <line x1="2" y1="12" x2="22" y2="12" />
                                <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
                              </svg>
                            </a>
                          )}
                        </div>
                      )}
                    </div>
                    {/* Dev Address - smaller, with star button */}
                    <div className="flex items-center gap-2">
                      <button
                        onClick={handleCopyCreator}
                        className="flex items-center gap-1 text-white/40 hover:text-white/70 font-mono text-xs transition-colors group cursor-pointer"
                        title="Copy creator address"
                      >
                        <span className="text-white/30">dev:</span>
                        <span>{shortenAddress(token.creator)}</span>
                        <VerifiedBadge wallet={token.creator} />
                        {copiedCreator ? (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="10"
                            height="10"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            className="text-success"
                          >
                            <polyline points="20 6 9 17 4 12" />
                          </svg>
                        ) : (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="10"
                            height="10"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            className="opacity-50 group-hover:opacity-100"
                          >
                            <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                          </svg>
                        )}
                      </button>
                      {/* Star button next to dev */}
                      <button
                        onClick={handleStarToken}
                        disabled={
                          !wallet.publicKey || token.hasStarred || token.starLoading || isOwnToken
                        }
                        className={`flex items-center gap-1 px-1.5 py-0.5 rounded text-xs transition-all ${
                          token.hasStarred
                            ? 'bg-yellow-500/20 text-accent cursor-default'
                            : wallet.publicKey && !isOwnToken
                              ? 'bg-white/10 hover:bg-yellow-500/20 text-white/70 hover:text-accent cursor-pointer'
                              : 'bg-white/5 text-white/30 cursor-not-allowed'
                        }`}
                        title={
                          !wallet.publicKey
                            ? 'Connect wallet to star'
                            : isOwnToken
                              ? 'Cannot star your own token'
                              : token.hasStarred
                                ? 'Already starred'
                                : 'Star this token (0.02 SOL)'
                        }
                      >
                        {token.starLoading ? (
                          <svg
                            className="animate-spin h-3 w-3"
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 24 24"
                          >
                            <circle
                              className="opacity-25"
                              cx="12"
                              cy="12"
                              r="10"
                              stroke="currentColor"
                              strokeWidth="4"
                            />
                            <path
                              className="opacity-75"
                              fill="currentColor"
                              d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                            />
                          </svg>
                        ) : token.hasStarred ? (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="10"
                            height="10"
                            viewBox="0 0 24 24"
                            fill="currentColor"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                          >
                            <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                          </svg>
                        ) : (
                          <svg
                            xmlns="http://www.w3.org/2000/svg"
                            width="10"
                            height="10"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                          >
                            <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                          </svg>
                        )}
                        <span>{token.stars}</span>
                      </button>
                    </div>
                  </div>
                  {/* Token Description */}
                  {token.metadata?.description && (
                    <p className="text-white/40 text-xs mt-2 max-w-[280px] line-clamp-2">
                      {token.metadata.description}
                    </p>
                  )}
                </div>
              </div>

              {/* Middle: Supply (Left) + Treasury (Right) */}
              <div className="flex-1 lg:mx-4 card p-3 flex flex-col sm:flex-row gap-3 sm:gap-6">
                {/* Supply Stats - Left */}
                <div className="flex-1">
                  <div className="flex items-center gap-2 mb-2">
                    <h3 className="text-white/50 text-xs font-medium lowercase tracking-wide">
                      Supply
                    </h3>
                    {holdersCount !== null && (
                      <span className="text-white/40 text-[10px] bg-white/10 px-1.5 py-0.5 rounded">
                        {holdersCount} holders
                      </span>
                    )}
                  </div>
                  {token.isMigrated || token.isComplete ? (
                    <div className="space-y-2">
                      <div className="flex items-center gap-2 text-success text-sm">
                        <svg
                          xmlns="http://www.w3.org/2000/svg"
                          width="14"
                          height="14"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="2"
                          strokeLinecap="round"
                          strokeLinejoin="round"
                        >
                          <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
                          <polyline points="22 4 12 14.01 9 11.01" />
                        </svg>
                        <span className="font-medium">Graduated</span>
                        <span className="text-white/40 text-xs">
                          Raised {token.solRaised.toFixed(2)} SOL
                        </span>
                      </div>
                      <div className="space-y-1 text-xs">
                        {token.treasuryLockBalance > 0 && (
                          <div className="flex justify-between">
                            <span className="text-white/40">Locked in Treasury</span>
                            <span className="text-accent font-mono">
                              {formatTokenAmount(token.treasuryLockBalance)}
                            </span>
                          </div>
                        )}
                        <div className="flex justify-between">
                          <span className="text-white/40">In DEX Pool</span>
                          <span className="text-success font-mono">
                            {formatTokenAmount(
                              token.poolTokenBalance > 0
                                ? token.poolTokenBalance
                                : token.tokensInCurve,
                            )}
                          </span>
                        </div>
                      </div>
                    </div>
                  ) : (
                    <div className="space-y-2">
                      {/* Progress Bar */}
                      <div>
                        <div className="flex justify-between text-xs text-white/50 mb-1">
                          <span>
                            {token.solRaised.toFixed(2)} / {token.solTarget} SOL
                          </span>
                          <span>{progress.toFixed(1)}%</span>
                        </div>
                        <div className="progress-bar h-2">
                          <div
                            className="progress-bar-fill"
                            style={{ width: `${Math.min(progress, 100)}%` }}
                          />
                        </div>
                      </div>
                      <div className="space-y-1 text-xs">
                        {token.treasuryLockBalance > 0 && (
                          <div className="flex justify-between">
                            <span className="text-white/40">Locked in Treasury</span>
                            <span className="text-accent font-mono">
                              {formatTokenAmount(token.treasuryLockBalance)}
                            </span>
                          </div>
                        )}
                        <div className="flex justify-between">
                          <span className="text-white/40">Available</span>
                          <span className="text-white font-mono">
                            {formatTokenAmount(token.tokensInCurve)}
                          </span>
                        </div>
                      </div>
                    </div>
                  )}

                  {/* Treasury Cranks (migrated tokens only) */}
                  {token.isMigrated && wallet.publicKey && (
                    <div className="pt-2 mt-2 border-t border-white/10 space-y-1.5">
                      {crankMsg && (
                        <p className={`text-xs ${crankMsg.startsWith('Error') ? 'text-danger' : 'text-success'}`}>
                          {crankMsg}
                        </p>
                      )}
                      <div className="flex gap-2">
                        <button
                          onClick={async () => {
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
                              setCrankMsg(allTxs.length > 1 ? 'Fees harvested & swapped (2 txs)' : 'Fees harvested & swapped to SOL')
                              setTimeout(() => { setCrankMsg(null); token.fetchToken() }, 3000)
                            } catch (e) {
                              const msg = (e as Error)?.message || 'Failed'
                              if (msg.includes('User rejected')) setCrankMsg('Cancelled')
                              else if (msg.includes('NoFeesToSwap') || msg.includes('insufficient') || msg.includes('No fees')) setCrankMsg('No fees to swap')
                              else if (msg.includes('Price below threshold') || msg.includes('below sell')) setCrankMsg('Price below sell threshold')
                              else if (msg.includes('cooldown') || msg.includes('Cooldown')) setCrankMsg('Cooldown not elapsed')
                              else setCrankMsg('Error: ' + msg.slice(0, 80))
                            } finally {
                              setCrankLoading(null)
                            }
                          }}
                          disabled={crankLoading !== null}
                          className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-white/10 text-white/70 hover:bg-white/20"
                        >
                          {crankLoading === 'harvest' ? 'Swapping...' : 'Harvest & Swap'}
                        </button>
                      </div>
                      <p className="text-white/30 text-[10px]">Permissionless — anyone can trigger</p>
                    </div>
                  )}
                </div>

                {/* Treasury Stats - Right */}
                <div className="flex-1">
                  <h3 className="text-white/50 text-xs font-medium mb-2 lowercase tracking-wide">
                    Treasury
                  </h3>
                  <div className="space-y-1 text-xs">
                    <div className="flex justify-between">
                      <span className="text-white/40">SOL Balance</span>
                      <span className="text-white font-mono">
                        {token.treasurySolBalance.toFixed(4)} SOL
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-white/40">Balance ({token.symbol})</span>
                      <span className="text-white font-mono">
                        {formatTokenAmount(token.treasuryTokenBalance)}
                      </span>
                    </div>
                    {token.isMigrated && token.mintWithheldFees > 0 && (
                      <div className="flex justify-between pt-1">
                        <span className="text-white/40">Harvestable Fees</span>
                        <span className="text-accent font-mono">
                          {formatTokenAmount(token.mintWithheldFees)}
                        </span>
                      </div>
                    )}
                  </div>

                  {/* SOL Distribution Bar (migrated tokens only) */}
                  {token.isMigrated && token.treasurySolBalance > 0 && (() => {
                    const total = token.treasurySolBalance
                    const reservePct = token.reserveRatioBps / 100
                    const reserve = total * reservePct / 100
                    const maxLendable = total * token.utilizationCapBps / 10000
                    const lent = token.totalSolLent
                    const available = Math.max(0, maxLendable - lent)

                    const lentPct = total > 0 ? (lent / total) * 100 : 0
                    const availPct = total > 0 ? (available / total) * 100 : 0
                    const reserveBarPct = total > 0 ? (reserve / total) * 100 : 0

                    return (
                      <div className="mt-2 pt-2 border-t border-white/10 space-y-1.5">
                        <p className="text-white/40 text-[10px] lowercase tracking-wide">SOL Distribution</p>
                        <div className="h-2 bg-white/5 rounded-full overflow-hidden flex">
                          {lentPct > 0 && (
                            <div className="bg-blue-500 h-full" style={{ width: `${lentPct}%` }} title={`Lent: ${lent.toFixed(2)} SOL`} />
                          )}
                          {availPct > 0 && (
                            <div className="bg-green-500/60 h-full" style={{ width: `${availPct}%` }} title={`Available: ${available.toFixed(2)} SOL`} />
                          )}
                          {reserveBarPct > 0 && (
                            <div className="bg-white/5 h-full" style={{ width: `${reserveBarPct}%` }} title={`Reserve: ${reserve.toFixed(2)} SOL`} />
                          )}
                        </div>
                        <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-[10px]">
                          {lent > 0 && (
                            <span className="flex items-center gap-1">
                              <span className="w-1.5 h-1.5 rounded-full bg-blue-500" />
                              <span className="text-white/40">Lent {lent.toFixed(2)}</span>
                            </span>
                          )}
                          <span className="flex items-center gap-1">
                            <span className="w-1.5 h-1.5 rounded-full bg-green-500/60" />
                            <span className="text-white/40">Lendable {available.toFixed(2)}</span>
                          </span>
                          <span className="flex items-center gap-1">
                            <span className="w-1.5 h-1.5 rounded-full bg-white/5 border border-white/10" />
                            <span className="text-white/40">Reserve {reserve.toFixed(2)}</span>
                          </span>
                        </div>
                      </div>
                    )
                  })()}

                </div>
              </div>

              {/* Right: Market Cap + Price (stacked) */}
              <div className="text-left lg:text-right lg:min-w-[140px]">
                <p className="text-white/40 text-sm">Market Cap</p>
                <p className="text-white font-mono text-xl lg:text-3xl font-bold whitespace-nowrap">
                  $
                  {(
                    (Number(token.marketCapLamports) / LAMPORTS_PER_SOL) *
                    (token.solPriceUsd || 0)
                  ).toLocaleString(undefined, { maximumFractionDigits: 0 })}
                </p>
                <p className="text-white/50 font-mono text-sm mt-1">
                  {token.priceInSol < 0.000001
                    ? token.priceInSol.toExponential(2)
                    : token.priceInSol.toFixed(8)}{' '}
                  SOL
                </p>
              </div>
            </div>
          </div>

          {/* === TAB BAR === */}
          <div className="flex gap-1 mb-3">
            <button
              onClick={() => setActiveTab('trading')}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer ${
                activeTab === 'trading'
                  ? 'bg-[color-mix(in_srgb,var(--accent)_18%,transparent)] text-[var(--accent)]'
                  : 'text-white/50 hover:bg-[var(--surface-hover)] hover:text-white/70'
              }`}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="22 7 13.5 15.5 8.5 10.5 2 17" />
                <polyline points="16 7 22 7 22 13" />
              </svg>
              Trading
            </button>
            <button
              onClick={() => setActiveTab('margin')}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer ${
                activeTab === 'margin'
                  ? 'bg-[color-mix(in_srgb,var(--accent)_18%,transparent)] text-[var(--accent)]'
                  : 'text-white/50 hover:bg-[var(--surface-hover)] hover:text-white/70'
              }`}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <line x1="12" y1="1" x2="12" y2="23" />
                <path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6" />
              </svg>
              Margin
            </button>
          </div>

          {/* === TAB CONTENT === */}
          {activeTab !== 'margin' ? (
            <TradingTab
              mintAddress={token.mintAddress}
              mint={token.mint!}
              userTokenBalance={token.userTokenBalance}
              vaultTokenBalance={vaultTokenBalance}
              symbol={token.symbol}
              isMigrated={token.isMigrated}
              isComplete={token.isComplete}
              isVoting={token.isVoting}
              isLegacy={isLegacyMint(token.mintAddress)}
              priceInSol={token.priceInSol}
              solRaised={token.solRaised}
              solPriceUsd={token.solPriceUsd}
              priceHistory={token.priceHistory}
              messages={token.messages}
              saidVerifications={saidVerifications}
              swapPreview={swapPreview}
              onPreviewChange={setSwapPreview}
              onTradeComplete={handleTradeComplete}
            />
          ) : (
            <LendingDashboard
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
      </main>

      {/* How It Works Modal */}
      <HowItWorksModal isOpen={showHowItWorks} onClose={() => setShowHowItWorks(false)} />

      {/* Protocol Treasury Modal */}
      <TreasuryModal isOpen={showTreasuryModal} onClose={() => setShowTreasuryModal(false)} />
    </div>
  )
}
