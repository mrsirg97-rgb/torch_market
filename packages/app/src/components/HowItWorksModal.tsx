'use client'

import { useEffect } from 'react'

interface HowItWorksModalProps {
  isOpen: boolean
  onClose: () => void
}

export function HowItWorksModal({ isOpen, onClose }: HowItWorksModalProps) {
  useEffect(() => {
    if (isOpen) {
      document.body.style.overflow = 'hidden'
    } else {
      document.body.style.overflow = ''
    }
    return () => { document.body.style.overflow = '' }
  }, [isOpen])

  if (!isOpen) return null

  return (
    <div
      className="fixed inset-0 modal-backdrop z-50 flex items-center justify-center p-4"
      onClick={onClose}
    >
      <div
        className="modal-surface p-6 sm:p-8 max-w-2xl w-full max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between mb-6">
          <h2 className="text-xl font-bold lowercase" style={{ color: 'var(--foreground)' }}>
            how torch.market works
          </h2>
          <button
            onClick={onClose}
            className="text-2xl cursor-pointer"
            style={{ color: 'var(--muted)' }}
          >
            &times;
          </button>
        </div>

        {/* Scrollable content — single page */}
        <div className="flex-1 overflow-y-auto pr-2 space-y-5">
          {/* What it is */}
          <p className="text-sm" style={{ color: 'var(--muted)' }}>
            Every market on torch is a self-contained margin venue. A funded treasury, a 300M
            reserve for short borrows, lending, and on-chain pricing — all live the moment the
            market migrates to its DEX pool.
          </p>

          {/* Lifecycle */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              market lifecycle
            </h3>
            <div className="space-y-2">
              {[
                { step: '1', title: 'Launch', desc: 'A new market mints 1B units of its asset — 700M to the bonding curve, 300M reserved for short borrows. Choose Flame (100 SOL target) or Torch (200 SOL).' },
                { step: '2', title: 'Bond', desc: 'Buyers swap SOL for the asset on the bonding curve. 17.5%→2.5% of incoming SOL routes to the treasury (decays as bonding fills). 2% max wallet.' },
                { step: '3', title: 'Migrate', desc: 'Anyone triggers migration. A Raydium pool is created, LP burned forever, mint/freeze authority revoked permanently.' },
                { step: '4', title: 'Trade', desc: 'The market trades on Raydium. A 0.07% transfer fee on every movement harvests to the treasury as SOL — the treasury grows perpetually.' },
                { step: '5', title: 'Margin', desc: 'Borrow SOL against your position as collateral. Short by posting SOL and borrowing from the 300M reserve. Max LTV scales with pool depth (30–60%), and a per-position size cap holds any position to 25% of pool depth.' },
              ].map(({ step, title, desc }) => (
                <div key={step} className="flex gap-3">
                  <div
                    className="w-6 h-6 rounded-full flex items-center justify-center flex-shrink-0 text-xs font-bold"
                    style={{ background: 'color-mix(in srgb, var(--accent) 15%, transparent)', color: 'var(--accent)' }}
                  >
                    {step}
                  </div>
                  <div>
                    <span className="text-sm font-medium" style={{ color: 'var(--foreground)' }}>{title}</span>
                    <span className="text-sm ml-1" style={{ color: 'var(--muted)' }}>— {desc}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Margin parameters */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              margin parameters
            </h3>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs" style={{ color: 'var(--muted)' }}>
              {[
                ['Max LTV', '30–60% (depth-scaled)'],
                ['Size Cap', '25% of pool depth'],
                ['Liquidation', '65%'],
                ['Interest', '1.5% / epoch (~7 days)'],
                ['Liq Bonus', '32.5% (depth-derived)'],
                ['Utilization Cap', '80%'],
                ['Per-User Cap', '23× share, 20% max'],
                ['Pricing', 'DeepPool TWAP (no external oracle)'],
                ['Positions', 'Isolated per user per market'],
              ].map(([label, value]) => (
                <div key={label} className="flex justify-between py-0.5">
                  <span>{label}</span>
                  <span className="font-mono" style={{ color: 'var(--foreground)' }}>{value}</span>
                </div>
              ))}
            </div>
          </div>

          {/* Vault */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              vault custody
            </h3>
            <p className="text-sm" style={{ color: 'var(--muted)' }}>
              The vault holds all SOL and positions. Your wallet (or agent) signs transactions
              but holds nothing of value. The authority can revoke any linked wallet instantly.
              Controllers cannot withdraw — ever.
            </p>
          </div>

          {/* Shorts explainer */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              short selling
            </h3>
            <p className="text-sm" style={{ color: 'var(--muted)' }}>
              Post SOL as collateral, borrow from the 300M reserve, and sell into the market.
              If price drops, buy back cheaper and keep the difference. Shorts are not synthetic
              — the borrow is real supply and selling it moves the real price.
            </p>
          </div>

          {/* Risk */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              risk
            </h3>
            <p className="text-sm" style={{ color: 'var(--muted)' }}>
              Positions can be liquidated. Bad debt is possible in extreme conditions and there
              is no insurance fund. But bad debt is isolated — one position going underwater
              cannot affect anyone else. Per-user caps prevent pool concentration. 20% of
              treasury SOL is always reserved.
            </p>
          </div>

          {/* Creator economics */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              creator economics
            </h3>
            <p className="text-sm" style={{ color: 'var(--muted)' }}>
              Community market (default): 100% of fees to treasury, zero creator extraction.
              Creator market (opt-in): 0.2%→1% SOL share during bonding, 85/15 treasury/creator
              on fee swaps, plus star payouts. The choice is permanent.
            </p>
          </div>

          {/* Protocol rewards */}
          <div>
            <h3 className="text-sm font-semibold mb-2 lowercase" style={{ color: 'var(--foreground)' }}>
              protocol rewards
            </h3>
            <p className="text-sm" style={{ color: 'var(--muted)' }}>
              0.5% of every bonding buy flows to the protocol treasury. Every ~7 days it
              distributes pro-rata to traders with 2+ SOL volume that epoch. Failed market
              reclaims also flow into rewards.
            </p>
          </div>
        </div>

        {/* Footer */}
        <div
          className="mt-4 pt-3 text-center text-xs"
          style={{ color: 'var(--muted)' }}
        >
          <a href="/whitepaper" className="hover:underline" style={{ color: 'var(--accent)' }}>Whitepaper</a>
          <span className="mx-2">|</span>
          <a href="/risk" className="hover:underline" style={{ color: 'var(--accent)' }}>Risk Model</a>
          <span className="mx-2">|</span>
          <a href="/tokenproposal" className="hover:underline" style={{ color: 'var(--accent)' }}>$torch</a>
          <span className="mx-2">|</span>
          <span>94 Kani proofs · All passing</span>
        </div>
      </div>
    </div>
  )
}
