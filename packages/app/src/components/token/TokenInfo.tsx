'use client'

import { useWallet } from '@solana/wallet-adapter-react'
import { useState } from 'react'
import Image from 'next/image'
import { shortenAddress, LAMPORTS_PER_SOL } from '@/lib/constants'
import { TokenMetadata } from '@/hooks/useToken'

interface TokenInfoProps {
  mintAddress: string
  name: string
  symbol: string
  priceInSol: number
  marketCapLamports: bigint
  solPriceUsd: number | null
  metadata: TokenMetadata | null
  isMigrated: boolean
  isComplete: boolean
  isVoting: boolean
  creator: string
  stars: number
  hasStarred: boolean
  starLoading: boolean
  onStarToken: () => void
}

export function TokenInfo({
  mintAddress,
  name,
  symbol,
  priceInSol,
  marketCapLamports,
  solPriceUsd,
  metadata,
  isMigrated,
  isComplete,
  isVoting,
  creator,
  stars,
  hasStarred,
  starLoading,
  onStarToken,
}: TokenInfoProps) {
  const wallet = useWallet()
  const [copied, setCopied] = useState(false)

  const truncateMiddle = (str: string, startChars = 8, endChars = 8) => {
    if (str.length <= startChars + endChars) return str
    return `${str.slice(0, startChars)}...${str.slice(-endChars)}`
  }

  const handleCopyMint = () => {
    navigator.clipboard.writeText(mintAddress)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const isOwnToken = wallet.publicKey?.toString() === creator

  const formatSolValue = (lamports: bigint) => {
    return (Number(lamports) / LAMPORTS_PER_SOL).toFixed(2)
  }

  return (
    <div className="card p-6 flex-1">
      <div className="flex items-start justify-between mb-4">
        <div className="flex items-start gap-4">
          {/* Token Image */}
          {metadata?.image ? (
            <Image
              src={metadata.image}
              alt={name}
              width={64}
              height={64}
              className="w-16 h-16 rounded-lg object-cover"
              unoptimized
            />
          ) : (
            <div className="w-16 h-16 rounded-lg bg-gradient-to-br from-accent/20 to-danger/20 flex items-center justify-center text-2xl">
              {symbol.charAt(0)}
            </div>
          )}
          <div>
            <h1 className="text-2xl lg:text-3xl font-bold text-white">{name}</h1>
            <p className="text-white/50">${symbol}</p>
          </div>
        </div>
        <span
          className={`badge ${
            isMigrated
              ? 'badge-migrated'
              : isComplete || isVoting
                ? 'badge-complete'
                : 'badge-bonding'
          }`}
        >
          {isMigrated ? 'Migrated' : isComplete || isVoting ? 'Complete' : 'Bonding'}
        </span>
      </div>

      {/* Description */}
      {metadata?.description && (
        <p className="text-white/60 text-sm mb-4">{metadata.description}</p>
      )}

      <div className="grid grid-cols-2 gap-4">
        <div>
          <p className="text-white/50 text-sm">Current Price</p>
          <p className="text-lg lg:text-xl font-mono text-white">
            {priceInSol < 0.000001 ? priceInSol.toExponential(2) : priceInSol.toFixed(8)} SOL
          </p>
        </div>
        <div>
          <p className="text-white/50 text-sm">Market Cap</p>
          <p className="text-lg lg:text-xl font-mono text-white">
            {solPriceUsd
              ? `$${((Number(marketCapLamports) / LAMPORTS_PER_SOL) * solPriceUsd).toLocaleString(undefined, { maximumFractionDigits: 0 })}`
              : `${formatSolValue(marketCapLamports)} SOL`}
          </p>
        </div>
      </div>

      {/* Social Links */}
      {(metadata?.twitter || metadata?.telegram || metadata?.website) && (
        <div className="mt-4 pt-4 border-t border-white/10 flex gap-3">
          {metadata.twitter && (
            <a
              href={metadata.twitter}
              target="_blank"
              rel="noopener noreferrer"
              className="text-white/50 hover:text-white transition-colors"
              title="Twitter"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
              </svg>
            </a>
          )}
          {metadata.telegram && (
            <a
              href={metadata.telegram}
              target="_blank"
              rel="noopener noreferrer"
              className="text-white/50 hover:text-white transition-colors"
              title="Telegram"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z" />
              </svg>
            </a>
          )}
          {metadata.website && (
            <a
              href={metadata.website}
              target="_blank"
              rel="noopener noreferrer"
              className="text-white/50 hover:text-white transition-colors"
              title="Website"
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
              >
                <circle cx="12" cy="12" r="10" />
                <line x1="2" y1="12" x2="22" y2="12" />
                <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
              </svg>
            </a>
          )}
        </div>
      )}

      <div className="mt-4 pt-4 border-t border-white/10 space-y-3">
        <div>
          <p className="text-white/50 text-sm mb-1">Contract Address</p>
          <button
            onClick={handleCopyMint}
            className="flex items-center gap-2 text-white/70 hover:text-white font-mono text-sm transition-colors group cursor-pointer"
            title="Copy contract address"
          >
            <span>{truncateMiddle(mintAddress, 12, 12)}</span>
            {copied ? (
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
                className="text-success"
              >
                <polyline points="20 6 9 17 4 12" />
              </svg>
            ) : (
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
                className="opacity-50 group-hover:opacity-100"
              >
                <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
              </svg>
            )}
          </button>
        </div>
        <div>
          <p className="text-white/50 text-sm mb-1">Creator</p>
          <div className="flex items-center gap-2">
            <span className="text-white/70 font-mono text-sm">
              {shortenAddress(creator)}
            </span>
            {/* Solscan link */}
            <a
              href={`https://solscan.io/account/${creator}`}
              target="_blank"
              rel="noopener noreferrer"
              className="text-white/40 hover:text-white/70 transition-colors"
              title="View on Solscan"
            >
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
                <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                <polyline points="15 3 21 3 21 9" />
                <line x1="10" y1="14" x2="21" y2="3" />
              </svg>
            </a>
            {/* Star button */}
            <button
              onClick={onStarToken}
              disabled={!wallet.publicKey || hasStarred || starLoading || isOwnToken}
              className={`flex items-center gap-1 px-2 py-1 rounded text-xs transition-all ${
                hasStarred
                  ? 'bg-yellow-500/20 text-yellow-400 cursor-default'
                  : wallet.publicKey && !isOwnToken
                    ? 'bg-white/10 hover:bg-yellow-500/20 text-white/70 hover:text-yellow-400 cursor-pointer'
                    : 'bg-white/5 text-white/30 cursor-not-allowed'
              }`}
              title={
                !wallet.publicKey
                  ? 'Connect wallet to star'
                  : isOwnToken
                    ? 'Cannot star your own token'
                    : hasStarred
                      ? 'Already starred'
                      : 'Star this token (0.02 SOL)'
              }
            >
              {starLoading ? (
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
              ) : hasStarred ? (
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  width="12"
                  height="12"
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
                  width="12"
                  height="12"
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
              <span>{stars}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
