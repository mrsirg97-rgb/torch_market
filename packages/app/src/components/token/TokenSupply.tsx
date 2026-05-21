'use client'

interface TokenSupplyProps {
  progress: number
  isMigrated: boolean
  isComplete: boolean
  solRaised: number
  solTarget?: number
  tokensInCurve: number
}

export function TokenSupply({
  progress,
  isMigrated,
  isComplete,
  solRaised,
  solTarget = 200,
  tokensInCurve,
}: TokenSupplyProps) {
  const formatNum = (n: number) =>
    n.toLocaleString(undefined, { maximumFractionDigits: 0 })

  if (isMigrated || isComplete) {
    return (
      <div className="card p-4">
        <h2 className="font-semibold mb-4">Token Distribution</h2>
        <div className="mb-4">
          <div className="flex items-center justify-center gap-2 text-teal-400 mb-3">
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
              <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
              <polyline points="22 4 12 14.01 9 11.01" />
            </svg>
            <span className="font-medium">Graduated to DEX</span>
          </div>
          <p className="text-white/40 text-xs text-center">
            Raised {solRaised.toFixed(2)} SOL
          </p>
        </div>

        <div className="text-center">
          <div className="bg-teal-500/10 rounded-lg p-3">
            <p className="text-white/50 text-xs mb-1">In DEX Pool</p>
            <p className="text-teal-400 font-mono text-sm font-medium">{formatNum(tokensInCurve)}</p>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="card p-4">
      <h2 className="font-semibold mb-3 text-sm">Token Supply</h2>
      <div className="mb-3">
        <div className="flex justify-between text-xs text-white/50 mb-1">
          <span>{solRaised.toFixed(2)} SOL</span>
          <span>{progress.toFixed(1)}%</span>
        </div>
        <div className="progress-bar h-2">
          <div className="progress-bar-fill" style={{ width: `${Math.min(progress, 100)}%` }} />
        </div>
        <p className="text-white/40 text-xs mt-1">Target: {solTarget} SOL</p>
      </div>
      <div className="space-y-1 text-xs">
        <div className="flex justify-between">
          <span className="text-white/50">Available</span>
          <span className="text-white font-mono">{formatNum(tokensInCurve)}</span>
        </div>
      </div>
    </div>
  )
}
