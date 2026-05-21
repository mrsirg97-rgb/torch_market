'use client'

import { useState } from 'react'
import { PublicKey } from '@solana/web3.js'
import { useWallet } from '@solana/wallet-adapter-react'
import { SwapPanel, UserPosition, ChartTabs, type SwapPreview } from '@/components/token'
import {
  formatSol,
  formatTokens,
  shortenAddress,
} from '@/lib/constants'
import { VerifiedBadgeInline } from '@/components'
import type { PricePoint } from '@/lib/trades'

interface Message {
  signature: string
  sender: string
  memo: string
  timestamp?: number
}

interface TradingTabProps {
  mintAddress: string
  mint: PublicKey
  userTokenBalance: bigint
  vaultTokenBalance: bigint | null
  symbol: string
  isMigrated: boolean
  isComplete: boolean
  isVoting: boolean
  isLegacy: boolean
  priceInSol: number
  solRaised: number
  solPriceUsd: number | null
  priceHistory: PricePoint[]
  messages: Message[]
  saidVerifications: Map<string, { verified: boolean; trustTier: 'high' | 'medium' | 'low' | null }>
  swapPreview: SwapPreview
  onPreviewChange: (preview: SwapPreview) => void
  onTradeComplete: () => void
}

export function TradingTab({
  mintAddress,
  mint,
  userTokenBalance,
  vaultTokenBalance,
  symbol,
  isMigrated,
  isComplete,
  isVoting,
  isLegacy,
  priceInSol,
  solRaised,
  solPriceUsd,
  priceHistory,
  messages,
  saidVerifications,
  swapPreview,
  onPreviewChange,
  onTradeComplete,
}: TradingTabProps) {
  const wallet = useWallet()
  const [activeTab, setActiveTab] = useState<'messages' | 'trades'>('messages')

  return (
    <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
      {/* Left Column (1/3): Swap + Position stacked */}
      <div className="flex flex-col gap-3">
        {/* Swap Panel */}
        <div className="card">
          <SwapPanel
            mintAddress={mintAddress}
            userTokenBalance={userTokenBalance}
            vaultTokenBalance={vaultTokenBalance}
            symbol={symbol}
            isMigrated={isMigrated}
            isComplete={isComplete}
            isVoting={isVoting}
            isLegacy={isLegacy}
            onTradeComplete={onTradeComplete}
            onPreviewChange={onPreviewChange}
          />
        </div>

        {/* User Position + Preview */}
        <div className="card p-3">
          {/* Swap Preview */}
          {swapPreview && (
            <div className="bg-white/5 rounded-lg p-3 text-sm mb-3">
              <h4 className="text-white/50 text-sm mb-2">Trade Preview</h4>
              {swapPreview.type === 'buy' ? (
                <>
                  <div className="flex justify-between mb-1">
                    <span className="text-white/50">You receive:</span>
                    <span className="text-white">
                      {formatTokens(swapPreview.tokensToUser)} {swapPreview.symbol}
                    </span>
                  </div>
                  <div className="flex justify-between mb-1">
                    <span className="text-white/50">Min. guaranteed:</span>
                    <span className="text-success">
                      {formatTokens(swapPreview.minGuaranteed)} {swapPreview.symbol}
                    </span>
                  </div>
                  <div className="flex justify-between mb-1">
                    <span className="text-white/50">To treasury:</span>
                    <span className="text-accent">
                      {formatTokens(swapPreview.tokensToCommunity)} {swapPreview.symbol}
                    </span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-white/50">Protocol fee:</span>
                    <span className="text-white/70">
                      {formatSol(swapPreview.protocolFee)} SOL
                    </span>
                  </div>
                </>
              ) : (
                <>
                  <div className="flex justify-between mb-1">
                    <span className="text-white/50">You receive:</span>
                    <span className="text-white">{formatSol(swapPreview.solToUser)} SOL</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-white/50">Min. guaranteed:</span>
                    <span className="text-success">
                      {formatSol(swapPreview.minGuaranteed)} SOL
                    </span>
                  </div>
                </>
              )}
            </div>
          )}

          {/* User Position */}
          {wallet.publicKey ? (
            <UserPosition
              userTokenBalance={userTokenBalance}
              vaultTokenBalance={vaultTokenBalance}
              symbol={symbol}
              priceHistory={priceHistory}
              walletAddress={wallet.publicKey.toBase58()}
            />
          ) : (
            <div className="text-center py-4">
              <p className="text-white/40 text-sm">Connect wallet to see position</p>
            </div>
          )}
        </div>
      </div>

      {/* Right Column (2/3): Chart + Messages stacked */}
      <div className="lg:col-span-2 flex flex-col gap-3">
        {/* Chart/Holders/Bubbles - Fixed height on desktop */}
        <div className="lg:h-[400px] card p-3">
          <ChartTabs mint={mint} priceInSol={priceInSol} solRaised={solRaised} solPriceUsd={solPriceUsd} priceHistory={priceHistory} />
        </div>

        {/* Messages / Trades */}
        <div className="card p-4">
          {/* Tabs */}
          <div className="flex gap-1 mb-3">
            <button
              onClick={() => setActiveTab('messages')}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer ${
                activeTab === 'messages'
                  ? 'bg-accent/20 text-accent'
                  : 'bg-white/5 text-white/50 hover:bg-white/10 hover:text-white/70'
              }`}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
              </svg>
              Messages
              <span className="text-xs font-normal opacity-60">({messages.length})</span>
            </button>
            <button
              onClick={() => setActiveTab('trades')}
              className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors cursor-pointer ${
                activeTab === 'trades'
                  ? 'bg-accent/20 text-accent'
                  : 'bg-white/5 text-white/50 hover:bg-white/10 hover:text-white/70'
              }`}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <polyline points="17 1 21 5 17 9" />
                <path d="M3 11V9a4 4 0 0 1 4-4h14" />
                <polyline points="7 23 3 19 7 15" />
                <path d="M21 13v2a4 4 0 0 1-4 4H3" />
              </svg>
              Trades
              <span className="text-xs font-normal opacity-60">({priceHistory.length})</span>
            </button>
          </div>

          {/* Messages Tab */}
          {activeTab === 'messages' && (
            messages.length > 0 ? (
              <div className="space-y-2 max-h-[300px] overflow-y-auto">
                {messages.map((msg) => (
                  <div key={msg.signature} className="bg-white/5 rounded-lg p-3">
                    <div className="flex items-start justify-between gap-2 mb-1">
                      <span className="text-accent text-xs font-mono flex items-center gap-1">
                        {shortenAddress(msg.sender)}
                        {saidVerifications.get(msg.sender)?.verified && (
                          <VerifiedBadgeInline
                            verified={true}
                            trustTier={saidVerifications.get(msg.sender)?.trustTier ?? null}
                          />
                        )}
                      </span>
                      <span className="text-white/30 text-xs">
                        {msg.timestamp
                          ? new Date(msg.timestamp * 1000).toLocaleString(undefined, {
                              month: 'short',
                              day: 'numeric',
                              hour: '2-digit',
                              minute: '2-digit',
                            })
                          : ''}
                      </span>
                    </div>
                    <p className="text-white text-sm">{msg.memo}</p>
                    <a
                      href={`https://solscan.io/tx/${msg.signature}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-white/30 hover:text-white/50 text-xs mt-1 inline-block"
                    >
                      View tx
                    </a>
                  </div>
                ))}
              </div>
            ) : (
              <div className="text-center py-4">
                <p className="text-white/40 text-sm">No messages yet</p>
                <p className="text-white/30 text-xs mt-1">
                  Trade with a message to start the conversation
                </p>
              </div>
            )
          )}

          {/* Trades Tab */}
          {activeTab === 'trades' && (
            priceHistory.length > 0 ? (
              <div className="space-y-2 max-h-[300px] overflow-y-auto">
                {[...priceHistory].reverse().map((trade, i) => (
                  <div key={i} className="bg-white/5 rounded-lg p-3">
                    <div className="flex items-center justify-between gap-2 mb-1">
                      <div className="flex items-center gap-2">
                        <span className={`text-xs font-semibold ${trade.isBuy ? 'text-success' : 'text-danger'}`}>
                          {trade.isBuy ? 'BUY' : 'SELL'}
                        </span>
                        <a
                          href={`https://solscan.io/account/${trade.trader}`}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="text-accent text-xs font-mono hover:underline"
                        >
                          {shortenAddress(trade.trader)}
                        </a>
                      </div>
                      <span className="text-white/30 text-xs">
                        {new Date(trade.timestamp * 1000).toLocaleString(undefined, {
                          month: 'short',
                          day: 'numeric',
                          hour: '2-digit',
                          minute: '2-digit',
                        })}
                      </span>
                    </div>
                    <div className="flex items-center gap-3 text-xs">
                      <span className="text-white/50">
                        Vol: <span className="text-white">{trade.volume.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 })} SOL</span>
                      </span>
                      <span className="text-white/50">
                        Price: <span className="text-white">{trade.price.toFixed(10)} SOL</span>
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <div className="text-center py-4">
                <p className="text-white/40 text-sm">No trades yet</p>
              </div>
            )
          )}
        </div>
      </div>
    </div>
  )
}
