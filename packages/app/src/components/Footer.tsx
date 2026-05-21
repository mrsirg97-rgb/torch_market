'use client'

import { useNetwork } from '@/lib/NetworkContext'

export function Footer() {
  const { networkId, isMainnet } = useNetwork()

  return (
    <footer className="border-t border-white/10 py-1">
      <div className="max-w-7xl mx-auto px-4 flex items-center justify-center gap-2">
        {!isMainnet && (
          <span
            className={`px-2 py-0.5 text-[10px] font-medium rounded border ${
              networkId === 'simnet'
                ? 'bg-purple-500/20 text-purple-300 border-purple-500/30'
                : 'bg-yellow-500/20 text-yellow-300 border-yellow-500/30'
            }`}
          >
            {networkId === 'devnet' ? 'Devnet' : networkId === 'simnet' ? 'Simnet' : 'Local'}
          </span>
        )}

        <span className="text-black/30 text-[10px] font-medium">Brightside Solutions</span>

        <a
          href="https://torch-market-docs.vercel.app"
          target="_blank"
          rel="noopener noreferrer"
          className="w-6 h-6 rounded-full flex items-center justify-center bg-yellow-500 hover:bg-yellow-400 transition-all cursor-pointer"
          title="Documentation"
        >
          <span className="text-[10px]">✌️</span>
        </a>

        <a
          href="https://pump.fun/coin/Rfe9sg18cPCPzpxBj6VzTomANPpDeUDc5w7RdSXpump"
          target="_blank"
          rel="noopener noreferrer"
          className="w-6 h-6 rounded-full flex items-center justify-center bg-black hover:bg-gray-900 transition-all cursor-pointer border border-white/20"
          title="TORCH on Pump.fun"
        >
          <svg
            width="16"
            height="10"
            viewBox="0 0 24 12"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
          >
            <rect x="0" y="0" width="24" height="12" rx="6" fill="#86efac" />
            <rect x="12" y="0" width="12" height="12" rx="6" fill="#ffffff" />
          </svg>
        </a>
      </div>
    </footer>
  )
}
