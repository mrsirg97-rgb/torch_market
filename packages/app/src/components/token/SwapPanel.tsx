'use client'

import { useState, useEffect, useRef, useCallback } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { useMwaSendTransaction } from '@/hooks/useMwaSendTransaction'
import {
  getBuyQuote,
  getSellQuote,
  buildBuyTransaction,
  buildDirectBuyTransaction,
  buildSellTransaction,
  buildMigrateTransaction,
  getVault,
  applySlippageBps,
} from 'torchsdk'
import type { BuyQuoteResult, SellQuoteResult, VaultInfo } from 'torchsdk'
import {
  LAMPORTS_PER_SOL,
  formatSol,
  formatTokens,
  TOKEN_MULTIPLIER,
  MAX_WALLET_TOKENS,
} from '@/lib/constants'

const isDev = process.env.NODE_ENV === 'development'

// Helper to confirm transaction with graceful timeout handling
async function confirmTransactionSafe(
  connection: ReturnType<typeof useConnection>['connection'],
  signature: string,
  blockhash: string,
  lastValidBlockHeight: number,
): Promise<void> {
  try {
    const confirmation = await connection.confirmTransaction(
      {
        signature,
        blockhash,
        lastValidBlockHeight,
      },
      'confirmed',
    )
    if (confirmation.value.err) {
      // Try to fetch logs for a more useful error message
      try {
        const tx = await connection.getTransaction(signature, {
          commitment: 'confirmed',
          maxSupportedTransactionVersion: 0,
        })
        if (tx?.meta?.logMessages) {
          const errorLog = tx.meta.logMessages.find(
            (log) => log.includes('Error Message:') || log.includes('AnchorError'),
          )
          if (errorLog) {
            const msgMatch = errorLog.match(/Error Message:\s*(.+)/)
            if (msgMatch) throw new Error(msgMatch[1])
            // Try AnchorError format
            const anchorMatch = errorLog.match(/AnchorError.*Error Code: (\w+)/)
            if (anchorMatch) throw new Error(anchorMatch[1])
          }
        }
      } catch (logErr) {
        // If this is an error we extracted above, re-throw it
        if (logErr instanceof Error && !logErr.message.includes('fetch'))
          throw logErr
      }
      throw new Error('Transaction failed on-chain')
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    const errName = err instanceof Error ? err.name : ''

    // If it's a timeout/expiration error, the tx may still succeed - don't throw
    if (
      message.includes('block height exceeded') ||
      message.includes('TransactionExpiredBlockheightExceededError') ||
      errName.includes('TransactionExpired')
    ) {
      return // Don't throw - tx was sent successfully
    }
    throw err
  }
}

// Export preview type for use in parent components
export type SwapPreview =
  | {
      type: 'buy'
      tokensToUser: bigint
      tokensToCommunity: bigint
      solToTreasury: bigint
      protocolFee: bigint
      minGuaranteed: bigint
      symbol: string
    }
  | {
      type: 'sell'
      solToUser: bigint
      minGuaranteed: bigint
    }
  | null

interface SwapPanelProps {
  mintAddress: string
  isMigrated: boolean
  isComplete: boolean
  isVoting: boolean // bonding complete but not migrated
  isLegacy: boolean // legacy token — withdraw (sell) only
  userTokenBalance: bigint
  /** Vault's balance of this mint, if the connected wallet has a vault. */
  vaultTokenBalance?: bigint | null
  symbol: string
  onTradeComplete: () => void
  onPreviewChange?: (preview: SwapPreview) => void
}

// Parse error messages into user-friendly text
function parseErrorMessage(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err)

  if (message.includes('SlippageExceeded')) {
    return 'Price moved too fast. Try increasing slippage or try again.'
  }
  if (message.includes('MaxWalletExceeded')) {
    return 'This would exceed the 2% max wallet limit.'
  }
  if (message.includes('InsufficientTokens')) {
    return 'Not enough tokens available in the pool.'
  }
  if (message.includes('InsufficientSol')) {
    return 'Not enough SOL in the pool for this trade.'
  }
  if (message.includes('InsufficientUserBalance')) {
    return "You don't have enough tokens to sell."
  }
  if (message.includes('BondingComplete')) {
    return 'Bonding phase is complete. Trade on DEX instead.'
  }
  if (message.includes('BondingNotComplete')) {
    return 'Bonding phase not complete yet.'
  }
  if (message.includes('AmountTooSmall')) {
    return 'Amount too small. Minimum is 0.001 SOL.'
  }
  if (message.includes('ProtocolPaused')) {
    return 'Protocol is temporarily paused.'
  }
  if (message.includes('AlreadyVoted')) {
    return 'You have already voted on this market.'
  }
  if (message.includes('User rejected')) {
    return 'Transaction cancelled.'
  }
  if (
    message.includes('AccountNotInitialized') ||
    message.includes('already initialized') ||
    message.includes('expected this account to be already initialized')
  ) {
    return 'Account not found. If you received tokens via transfer, try a small buy first to initialize your position.'
  }
  if (message.includes('insufficient funds') || message.includes('Insufficient funds')) {
    return 'Insufficient SOL balance for this transaction.'
  }
  if (message.includes('blockhash not found')) {
    return 'Transaction expired. Please try again.'
  }

  // "Already processed" means the transaction actually succeeded
  if (message.includes('already been processed')) {
    return '' // Return empty to indicate success (tx went through)
  }

  if (message.includes('Simulation failed')) {
    const match = message.match(/Error Code: (\w+)/)
    if (match) {
      return parseErrorMessage(new Error(match[1]))
    }
  }

  // Confirmation timeout/expiration - transaction may have succeeded
  if (
    message.includes('block height exceeded') ||
    message.includes('TransactionExpiredBlockheightExceededError')
  ) {
    return 'Transaction sent but confirmation timed out. Check your wallet for status.'
  }

  return message.length > 100 ? 'Transaction failed. Please try again.' : message
}

export function SwapPanel({
  mintAddress,
  isMigrated,
  isComplete,
  isVoting,
  isLegacy,
  userTokenBalance,
  vaultTokenBalance,
  symbol,
  onTradeComplete,
  onPreviewChange,
}: SwapPanelProps) {
  const { connection } = useConnection()
  const wallet = useWallet()
  const sendTransaction = useMwaSendTransaction()

  // Vault state
  const [userVault, setUserVault] = useState<VaultInfo | null>(null)
  const [useVault, setUseVault] = useState(false)

  // Detect user's vault on wallet connect
  useEffect(() => {
    if (!wallet.publicKey) {
      setUserVault(null)
      setUseVault(false)
      return
    }
    let cancelled = false
    getVault(connection, wallet.publicKey.toString())
      .then((v) => { if (!cancelled) setUserVault(v) })
      .catch(() => { if (!cancelled) setUserVault(null) })
    return () => { cancelled = true }
  }, [connection, wallet.publicKey])

  // Trade form state
  const [tradeTab, setTradeTab] = useState<'buy' | 'sell'>('buy')
  const [amount, setAmount] = useState('')
  const [slippageBpsRaw, setSlippageBpsRaw] = useState(100)
  const slippageBps = Math.max(10, Math.min(1000, slippageBpsRaw))
  const setSlippageBps = (value: number) => setSlippageBpsRaw(Math.max(10, Math.min(1000, value)))
  const [memo, setMemo] = useState('')

  // UI state
  const [actionLoading, setActionLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [errorTxId, setErrorTxId] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // Async preview state (replaces synchronous useMemo).
  // SDK's getBuyQuote / getSellQuote auto-routes bonding vs DeepPool DEX via
  // quote.source — so a single preview path covers both phases.
  const [buyQuote, setBuyQuote] = useState<BuyQuoteResult | null>(null)
  const [sellQuote, setSellQuote] = useState<SellQuoteResult | null>(null)
  const [previewLoading, setPreviewLoading] = useState(false)
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // Debounced async preview fetch
  const fetchPreview = useCallback(
    (currentAmount: string, currentTab: 'buy' | 'sell') => {
      // Clear previous debounce
      if (debounceRef.current) {
        clearTimeout(debounceRef.current)
      }

      // Clear stale quotes
      setBuyQuote(null)
      setSellQuote(null)

      if (!currentAmount) {
        setPreviewLoading(false)
        return
      }

      const parsedAmount = parseFloat(currentAmount)
      if (isNaN(parsedAmount) || parsedAmount <= 0) {
        setPreviewLoading(false)
        return
      }

      setPreviewLoading(true)

      debounceRef.current = setTimeout(async () => {
        try {
          if (currentTab === 'buy') {
            const lamports = Math.floor(parsedAmount * LAMPORTS_PER_SOL)
            const quote = await getBuyQuote(connection, mintAddress, lamports)
            setBuyQuote(quote)
            setSellQuote(null)
          } else {
            const tokenUnits = Math.floor(parsedAmount * TOKEN_MULTIPLIER)
            const quote = await getSellQuote(connection, mintAddress, tokenUnits)
            setSellQuote(quote)
            setBuyQuote(null)
          }
        } catch (err) {
          if (isDev) console.error('Preview fetch failed:', err)
          setBuyQuote(null)
          setSellQuote(null)
        } finally {
          setPreviewLoading(false)
        }
      }, 300)
    },
    [connection, mintAddress],
  )

  // Trigger preview fetch when amount or tab changes
  useEffect(() => {
    fetchPreview(amount, tradeTab)
    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current)
      }
    }
  }, [amount, tradeTab, fetchPreview])

  // Notify parent of preview changes
  useEffect(() => {
    if (!onPreviewChange) return

    if (buyQuote) {
      const tokensToUserBigint = BigInt(Math.floor(buyQuote.tokens_to_user))
      const tokensToCommunityBigint = BigInt(0) // [V36] Vote vault removed — 100% to buyer
      const protocolFeeBigint = BigInt(Math.floor(buyQuote.protocol_fee_sol))
      // solToTreasury: derive from the total buy fee structure (protocol_fee_sol covers it)
      const solToTreasuryBigint = BigInt(0) // SDK doesn't expose treasury SOL split separately
      onPreviewChange({
        type: 'buy',
        tokensToUser: tokensToUserBigint,
        tokensToCommunity: tokensToCommunityBigint,
        solToTreasury: solToTreasuryBigint,
        protocolFee: protocolFeeBigint,
        // [prompt-008 F-3] signed floor == displayed floor: one haircut on the raw output.
        minGuaranteed: applySlippageBps(BigInt(Math.floor(buyQuote.tokens_to_user)), slippageBps),
        symbol,
      })
    } else if (sellQuote) {
      const solToUserBigint = BigInt(Math.floor(sellQuote.output_sol))
      onPreviewChange({
        type: 'sell',
        solToUser: solToUserBigint,
        minGuaranteed: applySlippageBps(BigInt(Math.floor(sellQuote.output_sol)), slippageBps),
      })
    } else {
      onPreviewChange(null)
    }
  }, [buyQuote, sellQuote, symbol, onPreviewChange, slippageBps])

  // Handle bonding curve buy via SDK
  // [V36] Vote parameter removed — 100% of tokens to buyer
  async function handleBuy() {
    if (!wallet.publicKey) return

    const solAmount = parseFloat(amount)
    if (isNaN(solAmount) || solAmount <= 0) {
      setError('Enter a valid SOL amount')
      return
    }

    setActionLoading(true)
    setError(null)
    setErrorTxId(null)
    setSuccess(null)

    try {
      const lamports = Math.floor(solAmount * LAMPORTS_PER_SOL)

      const baseBuyParams = {
        buyer: wallet.publicKey.toString(),
        mint: mintAddress,
        amount_sol: lamports,
        slippage_bps: slippageBps,
        message: memo.trim() || undefined,
      }

      const { transaction: tx, migrationTransaction } = useVault && userVault
        ? await buildBuyTransaction(connection, { ...baseBuyParams, vault: userVault.creator })
        : await buildDirectBuyTransaction(connection, baseBuyParams)

      const txId = await sendTransaction(tx)
      const latestBlockhash = await connection.getLatestBlockhash()
      await connection.confirmTransaction(
        { signature: txId, blockhash: latestBlockhash.blockhash, lastValidBlockHeight: latestBlockhash.lastValidBlockHeight },
        'confirmed',
      )

      // Handle migration tx if present (auto-migrate on threshold)
      if (migrationTransaction) {
        try {
          await sendTransaction(migrationTransaction)
        } catch {
          // Migration is best-effort — don't fail the buy
        }
      }

      setAmount('')
      setMemo('')
      setTimeout(onTradeComplete, 2000)
    } catch (err: unknown) {
      const errorMsg = parseErrorMessage(err)
      if (errorMsg === '') {
        setAmount('')
        setMemo('')
        setTimeout(onTradeComplete, 2000)
      } else {
        setError(errorMsg)
      }
    } finally {
      setActionLoading(false)
    }
  }

  // Handle bonding curve sell via SDK
  async function handleSell() {
    if (!wallet.publicKey) return

    const tokenAmount = parseFloat(amount)
    if (isNaN(tokenAmount) || tokenAmount <= 0) {
      setError('Enter a valid amount')
      return
    }

    setActionLoading(true)
    setError(null)
    setErrorTxId(null)
    setSuccess(null)

    let txId: string | undefined
    try {
      const tokenUnits = Math.floor(tokenAmount * TOKEN_MULTIPLIER)

      const { transaction: tx } = await buildSellTransaction(connection, {
        mint: mintAddress,
        seller: wallet.publicKey.toString(),
        amount_tokens: tokenUnits,
        slippage_bps: slippageBps,
        message: memo.trim() || undefined,
        vault: useVault && userVault ? userVault.creator : undefined,
      })

      txId = await sendTransaction(tx)

      const latestBlockhash = await connection.getLatestBlockhash()
      await confirmTransactionSafe(
        connection,
        txId,
        latestBlockhash.blockhash,
        latestBlockhash.lastValidBlockHeight,
      )

      setAmount('')
      setMemo('')
      setTimeout(onTradeComplete, 2000)
    } catch (err: unknown) {
      const errorMsg = parseErrorMessage(err)
      if (errorMsg === '') {
        setAmount('')
        setMemo('')
        setTimeout(onTradeComplete, 2000)
      } else {
        setError(errorMsg)
        if (txId) setErrorTxId(txId)
      }
    } finally {
      setActionLoading(false)
    }
  }

  // Determine if bonding is complete (either isComplete or isVoting)
  const bondingComplete = isComplete || isVoting

  // 2% wallet cap check: would this buy exceed the max?
  const wouldExceedWalletCap =
    tradeTab === 'buy' &&
    buyQuote !== null &&
    userTokenBalance + BigInt(Math.floor(buyQuote.tokens_to_user)) > MAX_WALLET_TOKENS

  return (
    <div className="p-4">
      {isLegacy ? (
        /* Legacy Token — Withdraw Only */
        <div className="text-center py-6">
          <div className="inline-flex items-center gap-2 bg-white/10 text-white/50 px-3 py-1.5 rounded-full text-sm font-medium mb-3">
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10" />
              <line x1="12" y1="8" x2="12" y2="12" />
              <line x1="12" y1="16" x2="12.01" y2="16" />
            </svg>
            Legacy market
          </div>
          <p className="text-white/40 text-xs mb-4">
            This market is no longer active. Withdraw only.
          </p>
          {wallet.publicKey && userTokenBalance > BigInt(0) ? (
            <div className="space-y-3">
              <div className="bg-white/5 rounded-lg p-3">
                <p className="text-white/50 text-xs mb-1">Your Balance</p>
                <p className="text-white font-mono">{formatTokens(userTokenBalance)} {symbol}</p>
              </div>
              <div>
                <label className="text-white/50 text-xs mb-1 block">Sell Amount ({symbol})</label>
                <input
                  type="text"
                  inputMode="decimal"
                  value={amount}
                  onChange={(e) => setAmount(e.target.value)}
                  placeholder="0"
                  className="w-full bg-white/5 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:ring-1 focus:ring-white/20"
                />
                <button
                  onClick={() => setAmount((Number(userTokenBalance) / 1e6).toString())}
                  className="text-accent text-xs mt-1 hover:underline cursor-pointer"
                >
                  Max
                </button>
              </div>
              <button
                onClick={handleSell}
                disabled={actionLoading || !amount}
                className="w-full bg-danger/80 hover:bg-danger text-white py-3 rounded-lg font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
              >
                {actionLoading ? 'Selling...' : 'Withdraw (Sell)'}
              </button>
              {error && (
                <div className="text-sm text-center text-danger">
                  <p>{error}</p>
                  {errorTxId && (
                    <a
                      href={`https://solscan.io/tx/${errorTxId}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-xs text-white/50 hover:text-white/70 underline mt-1 inline-block"
                    >
                      View transaction
                    </a>
                  )}
                </div>
              )}
              {success && <p className="text-sm text-center text-success">{success}</p>}
            </div>
          ) : wallet.publicKey ? (
            <p className="text-white/30 text-xs">You have no tokens to withdraw.</p>
          ) : (
            <p className="text-white/30 text-xs">Connect wallet to withdraw.</p>
          )}
        </div>
      ) : isMigrated ? (
        /* DEX Swap UI for Migrated Tokens — DeepPool (direct or vault-routed) */
        <>
          <div className="text-center mb-4">
            <p className="text-white/50 text-xs">Trade on DeepPool</p>
          </div>

          <div className="flex mb-4 border-b border-white/10">
            <button
              onClick={() => setTradeTab('buy')}
              className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
                tradeTab === 'buy'
                  ? 'text-success border-b-2 border-success'
                  : 'text-white/50 hover:text-white/70'
              }`}
            >
              Buy
            </button>
            <button
              onClick={() => setTradeTab('sell')}
              className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
                tradeTab === 'sell'
                  ? 'text-danger border-b-2 border-danger'
                  : 'text-white/50 hover:text-white/70'
              }`}
            >
              Sell
            </button>
          </div>

          <div className="space-y-4">
            <div>
              <label className="block text-sm text-white/50 mb-2">
                {tradeTab === 'buy' ? 'SOL Amount' : `Amount (${symbol})`}
              </label>
              <input
                type="number"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
                placeholder={tradeTab === 'buy' ? '0.0 SOL' : '0 tokens'}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              {/* Buy amount buttons */}
              {tradeTab === 'buy' && (
                <div className="flex gap-2 mt-2">
                  {[0.25, 0.5, 1, 2].map((sol) => (
                    <button
                      key={sol}
                      onClick={() => setAmount(sol.toString())}
                      className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                    >
                      {sol} SOL
                    </button>
                  ))}
                </div>
              )}
              {/* Sell percentage buttons — base on vault or wallet balance per the active route */}
              {tradeTab === 'sell' && (() => {
                const sellableBalance =
                  useVault && userVault && vaultTokenBalance && vaultTokenBalance > BigInt(0)
                    ? vaultTokenBalance
                    : userTokenBalance
                if (sellableBalance <= BigInt(0)) return null
                return (
                  <div className="flex gap-2 mt-2">
                    {[25, 50, 75, 100].map((pct) => (
                      <button
                        key={pct}
                        onClick={() => {
                          const tokenAmount = (sellableBalance * BigInt(pct)) / BigInt(100)
                          setAmount((Number(tokenAmount) / TOKEN_MULTIPLIER).toString())
                        }}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {pct === 100 ? 'Max' : `${pct}%`}
                      </button>
                    ))}
                  </div>
                )
              })()}
            </div>

            {/* Slippage Tolerance */}
            <div>
              <label className="block text-sm text-white/50 mb-2">Slippage Tolerance</label>
              <div className="flex gap-2">
                {[50, 100, 200, 500].map((bps) => (
                  <button
                    key={bps}
                    onClick={() => setSlippageBps(bps)}
                    className={`flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border ${
                      slippageBps === bps
                        ? 'border-accent text-white bg-white/5'
                        : 'border-transparent bg-white/10 text-white/70 hover:bg-white/20'
                    }`}
                  >
                    {bps / 100}%
                  </button>
                ))}
              </div>
            </div>

            {/* Message (Memo) */}
            <div>
              <label className="block text-sm text-white/50 mb-2">Message (optional)</label>
              <input
                type="text"
                value={memo}
                onChange={(e) => setMemo(e.target.value)}
                placeholder="Say something..."
                maxLength={500}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-2 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              <p className="text-white/30 text-xs mt-1">Message stored on-chain with your trade</p>
            </div>

            {/* Vault Toggle — opt-in routing through vault when one is present */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Trade via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'SOL from vault, tokens to vault' : 'Direct wallet trade'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {/* DEX Quote Preview (driven by SDK quotes — same as bonding) */}
            {previewLoading && (
              <div className="bg-white/5 rounded-lg p-3 text-sm text-center text-white/50">
                Fetching quote...
              </div>
            )}
            {!previewLoading && (buyQuote || sellQuote) && (
              <div className="bg-white/5 rounded-lg p-3 text-sm">
                <div className="flex justify-between mb-1">
                  <span className="text-white/50">You receive:</span>
                  <span className="text-white">
                    {tradeTab === 'buy' && buyQuote
                      ? `${formatTokens(BigInt(Math.floor(buyQuote.tokens_to_user)))} ${symbol}`
                      : sellQuote
                        ? `${formatSol(BigInt(Math.floor(sellQuote.output_sol)))} SOL`
                        : ''}
                  </span>
                </div>
                <div className="flex justify-between">
                  <span className="text-white/50">Min. guaranteed:</span>
                  <span className="text-success">
                    {tradeTab === 'buy' && buyQuote
                      ? `${formatTokens(applySlippageBps(BigInt(Math.floor(buyQuote.tokens_to_user)), slippageBps))} ${symbol}`
                      : sellQuote
                        ? `${formatSol(applySlippageBps(BigInt(Math.floor(sellQuote.output_sol)), slippageBps))} SOL`
                        : ''}
                  </span>
                </div>
              </div>
            )}

            {/* Error/Success */}
            {error && (
              <div className="text-danger text-sm">
                <p>{error}</p>
                {errorTxId && (
                  <a
                    href={`https://solscan.io/tx/${errorTxId}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-xs text-white/50 hover:text-white/70 underline mt-1 inline-block"
                  >
                    View transaction
                  </a>
                )}
              </div>
            )}
            {success && <p className="text-success text-sm">{success}</p>}

            {/* Action Button */}
            {wallet.publicKey ? (
              <button
                onClick={tradeTab === 'buy' ? handleBuy : handleSell}
                disabled={actionLoading || !amount}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                style={{
                  backgroundColor: tradeTab === 'buy' ? '#22c55e' : '#ef4444',
                  color: tradeTab === 'buy' ? 'black' : 'white',
                }}
              >
                {actionLoading ? 'Processing...' : tradeTab === 'buy' ? 'Buy' : 'Sell'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to trade</p>
            )}
          </div>
        </>
      ) : isComplete ? (
        /* Bonding Complete - Migrate to DEX */
        <div className="space-y-4">
          <div className="text-center mb-4">
            <div className="inline-flex items-center gap-2 bg-green-500/20 text-success px-3 py-1.5 rounded-full text-sm font-medium mb-3">
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="16"
                height="16"
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
              Bonding Complete!
            </div>
            <p className="text-white/50 text-sm">Vote finalized. Ready for DEX migration.</p>
          </div>
          <div className="bg-white/5 rounded-lg p-4 text-center">
            <p className="text-white/70 text-sm mb-2">
              Bonding phase is complete. Migrate to DeepPool DEX to enable trading.
            </p>
            <p className="text-white/40 text-xs mb-4">
              Anyone can trigger migration. LP tokens are burned and liquidity is locked forever.
            </p>
            {error && (
              <div className="text-danger text-sm mb-2">
                <p>{error}</p>
                {errorTxId && (
                  <a
                    href={`https://solscan.io/tx/${errorTxId}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-xs text-white/50 hover:text-white/70 underline mt-1 inline-block"
                  >
                    View transaction
                  </a>
                )}
              </div>
            )}
            {success && <p className="text-success text-sm mb-2">{success}</p>}
            {wallet.publicKey ? (
              <button
                onClick={async () => {
                  if (!wallet.publicKey) return
                  setActionLoading(true)
                  setError(null)
                  setSuccess(null)
                  try {
                    const { transaction } = await buildMigrateTransaction(connection, {
                      mint: mintAddress,
                      payer: wallet.publicKey.toString(),
                    })
                    const txId = await sendTransaction(transaction)
                    const latestBlockhash = await connection.getLatestBlockhash()
                    await confirmTransactionSafe(
                      connection,
                      txId,
                      latestBlockhash.blockhash,
                      latestBlockhash.lastValidBlockHeight,
                    )
                    setSuccess('Migration successful! Refreshing...')
                    setTimeout(onTradeComplete, 3000)
                  } catch (err: unknown) {
                    const errorMsg = parseErrorMessage(err)
                    if (errorMsg === '') {
                      setSuccess('Migration successful! Refreshing...')
                      setTimeout(onTradeComplete, 3000)
                    } else {
                      setError(errorMsg)
                    }
                  } finally {
                    setActionLoading(false)
                  }
                }}
                disabled={actionLoading}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed bg-accent text-black"
              >
                {actionLoading ? 'Migrating...' : 'Migrate to DEX'}
              </button>
            ) : (
              <p className="text-center text-white/50 text-sm py-3">Connect wallet to migrate</p>
            )}
            <p className="text-white/30 text-xs mt-2">
              Permissionless — anyone can trigger. Payer fronts ~1 SOL, treasury reimburses automatically.
            </p>
          </div>
        </div>
      ) : (
        /* Buy/Sell Form for Active Bonding */
        <>
          <div className="flex mb-4 border-b border-white/10">
            <button
              onClick={() => setTradeTab('buy')}
              className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
                tradeTab === 'buy'
                  ? 'text-success border-b-2 border-success'
                  : 'text-white/50 hover:text-white/70'
              }`}
            >
              Buy
            </button>
            <button
              onClick={() => setTradeTab('sell')}
              className={`flex-1 pb-3 text-center transition-colors cursor-pointer ${
                tradeTab === 'sell'
                  ? 'text-danger border-b-2 border-danger'
                  : 'text-white/50 hover:text-white/70'
              }`}
            >
              Sell
            </button>
          </div>

          <div className="space-y-4">
            <div>
              <label className="block text-sm text-white/50 mb-2">
                {tradeTab === 'buy' ? 'SOL Amount' : `Amount (${symbol})`}
              </label>
              <input
                type="number"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
                placeholder={tradeTab === 'buy' ? '0.0 SOL' : '0 tokens'}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              {/* Buy amount buttons */}
              {tradeTab === 'buy' && (
                <div className="flex gap-2 mt-2">
                  {[0.25, 0.5, 1, 2].map((sol) => (
                    <button
                      key={sol}
                      onClick={() => setAmount(sol.toString())}
                      className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                    >
                      {sol} SOL
                    </button>
                  ))}
                </div>
              )}
              {/* Sell percentage buttons — base on vault or wallet balance per the active route */}
              {tradeTab === 'sell' && (() => {
                const sellableBalance =
                  useVault && userVault && vaultTokenBalance && vaultTokenBalance > BigInt(0)
                    ? vaultTokenBalance
                    : userTokenBalance
                if (sellableBalance <= BigInt(0)) return null
                return (
                  <div className="flex gap-2 mt-2">
                    {[25, 50, 75, 100].map((pct) => (
                      <button
                        key={pct}
                        onClick={() => {
                          const tokenAmount = (sellableBalance * BigInt(pct)) / BigInt(100)
                          setAmount((Number(tokenAmount) / TOKEN_MULTIPLIER).toString())
                        }}
                        className="flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border border-transparent bg-white/10 text-white/70 hover:bg-white/20"
                      >
                        {pct === 100 ? 'Max' : `${pct}%`}
                      </button>
                    ))}
                  </div>
                )
              })()}
            </div>

            {/* Slippage Tolerance */}
            <div>
              <label className="block text-sm text-white/50 mb-2">Slippage Tolerance</label>
              <div className="flex gap-2">
                {[50, 100, 200, 500].map((bps) => (
                  <button
                    key={bps}
                    onClick={() => setSlippageBps(bps)}
                    className={`flex-1 py-1.5 text-xs rounded-lg font-medium transition-colors cursor-pointer border ${
                      slippageBps === bps
                        ? 'border-accent text-white bg-white/5'
                        : 'border-transparent bg-white/10 text-white/70 hover:bg-white/20'
                    }`}
                  >
                    {bps / 100}%
                  </button>
                ))}
              </div>
              <p className="text-white/30 text-xs mt-1">
                Transaction will revert if price moves more than {slippageBps / 100}%
              </p>
            </div>

            {/* Message (Memo) */}
            <div>
              <label className="block text-sm text-white/50 mb-2">Message (optional)</label>
              <input
                type="text"
                value={memo}
                onChange={(e) => setMemo(e.target.value)}
                placeholder="Say something..."
                maxLength={500}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-2 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              <p className="text-white/30 text-xs mt-1">Message stored on-chain with your trade</p>
            </div>

            {/* Vault Toggle */}
            {userVault && (
              <div className="flex items-center justify-between bg-white/5 rounded-lg p-3">
                <div>
                  <p className="text-sm text-white/70">Trade via Vault</p>
                  <p className="text-xs text-white/40">
                    {useVault ? 'SOL from vault, tokens to vault' : 'Direct wallet trade'}
                  </p>
                </div>
                <button
                  onClick={() => setUseVault(!useVault)}
                  className={`relative w-10 h-5 rounded-full transition-colors cursor-pointer ${
                    useVault ? 'bg-accent' : 'bg-white/20'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      useVault ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            )}

            {/* 2% Wallet Cap Warning */}
            {wouldExceedWalletCap && (
              <div className="bg-danger/10 border border-danger/30 rounded-lg p-3 text-sm">
                <p className="text-danger">
                  This buy would exceed the 2% max wallet limit (20M tokens). Reduce your amount.
                </p>
              </div>
            )}

            {/* High Slippage Warning */}
            {slippageBps > 500 && (
              <div className="bg-danger/10 border border-danger/30 rounded-lg p-3 text-sm">
                <p className="text-danger">
                  High slippage ({(slippageBps / 100).toFixed(1)}%) - you may receive significantly
                  fewer tokens than expected.
                </p>
              </div>
            )}

            {/* Error/Success */}
            {error && (
              <div className="text-danger text-sm">
                <p>{error}</p>
                {errorTxId && (
                  <a
                    href={`https://solscan.io/tx/${errorTxId}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-xs text-white/50 hover:text-white/70 underline mt-1 inline-block"
                  >
                    View transaction
                  </a>
                )}
              </div>
            )}
            {success && <p className="text-success text-sm">{success}</p>}

            {/* Action Button */}
            {tradeTab === 'buy' ? (
                <button
                  onClick={() => handleBuy()}
                  disabled={actionLoading || !wallet.publicKey || bondingComplete || wouldExceedWalletCap}
                  className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  style={{ backgroundColor: '#22c55e', color: 'black' }}
                >
                  {actionLoading
                    ? 'Processing...'
                    : !wallet.publicKey
                      ? 'Connect Wallet'
                      : bondingComplete
                        ? 'Bonding Complete'
                        : wouldExceedWalletCap
                          ? 'Exceeds 2% Wallet Cap'
                          : 'Buy Tokens'}
                </button>
            ) : (
              <button
                onClick={handleSell}
                disabled={actionLoading || !wallet.publicKey || bondingComplete}
                className="w-full py-3 text-sm rounded-lg font-semibold transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                style={{ backgroundColor: '#ef4444', color: 'white' }}
              >
                {actionLoading
                  ? 'Processing...'
                  : !wallet.publicKey
                    ? 'Connect Wallet'
                    : bondingComplete
                      ? 'Bonding Complete'
                      : 'Sell Tokens'}
              </button>
            )}

            {bondingComplete && (
              <p className="text-white/40 text-xs text-center">Trading disabled after bonding</p>
            )}
          </div>
        </>
      )}
    </div>
  )
}
