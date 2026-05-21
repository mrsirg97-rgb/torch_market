'use client'

import { useMemo } from 'react'
import { formatTokens } from '@/lib/constants'
import type { PricePoint } from '@/lib/trades'

interface UserPositionProps {
  userTokenBalance: bigint
  vaultTokenBalance: bigint | null
  symbol: string
  priceHistory: PricePoint[]
  walletAddress: string
}

export function UserPosition({ userTokenBalance, vaultTokenBalance, symbol, priceHistory, walletAddress }: UserPositionProps) {
  const userTrades = useMemo(
    () => priceHistory.filter(p => p.trader === walletAddress),
    [priceHistory, walletAddress],
  )

  return (
    <div>
      <h3 className="font-semibold mb-3 text-sm">Your Position</h3>
      <div className="space-y-1 text-xs">
        <div className="flex justify-between">
          <span className="text-white/50">Wallet Balance:</span>
          <span className="text-white font-mono">
            {formatTokens(userTokenBalance)} {symbol}
          </span>
        </div>
        {vaultTokenBalance !== null && (
          <div className="flex justify-between">
            <span className="text-white/50">Vault Balance:</span>
            <span className="text-white font-mono">
              {formatTokens(vaultTokenBalance)} {symbol}
            </span>
          </div>
        )}
      </div>

      {/* Trade History */}
      <div className="mt-3 pt-3 border-t border-white/10">
        <h4 className="text-white/50 text-xs mb-2">Trade History</h4>
        {userTrades.length > 0 ? (
          <div className="space-y-1.5 max-h-[200px] overflow-y-auto">
            {userTrades.map((trade, i) => (
              <div key={`${trade.timestamp}-${i}`} className="bg-white/5 rounded-lg p-2 text-xs">
                <div className="flex items-center justify-between mb-0.5">
                  <span className={trade.isBuy ? 'text-green-400 font-semibold' : 'text-red-400 font-semibold'}>
                    {trade.isBuy ? 'Buy' : 'Sell'}
                  </span>
                  <span className="text-white/30">
                    {new Date(trade.timestamp * 1000).toLocaleString(undefined, {
                      month: 'short',
                      day: 'numeric',
                      hour: '2-digit',
                      minute: '2-digit',
                    })}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-white font-mono">{trade.volume.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 })} SOL</span>
                  <span className="text-white/50 font-mono">@ {trade.price.toFixed(10)} SOL</span>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-white/30 text-xs">No trades yet</p>
        )}
      </div>
    </div>
  )
}
