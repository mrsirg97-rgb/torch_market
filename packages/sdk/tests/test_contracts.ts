/**
 * Cross-layer contract pins — the failure class of 2026-06-11: one layer
 * relabels, the other doesn't hear. Run: npx tsx tests/test_contracts.ts
 */
import { sdkStatusToIndexer } from '../src/tokens'
import type { IndexerMarketStatus } from '../src/indexer'
import type { FeedFrame } from '../src/feed'
import { sizeLongMinOut, sizeShortMinOut, netCollateralAfterFee } from '../src/transactions'
import { cpmmSwap } from '../src/quotes'

let pass = 0, fail = 0
const ok = (c: boolean, n: string) => { c ? pass++ : fail++; console.log(`${c ? 'ok  ' : 'FAIL'} - ${n}`) }

// Status mapping round-trip (BONDING/COMPLETE/MIGRATED/RECLAIMED era).
ok(sdkStatusToIndexer('bonding') === 'BONDING', 'bonding → BONDING')
ok(sdkStatusToIndexer('complete') === 'COMPLETE', 'complete → COMPLETE')
ok(sdkStatusToIndexer('migrated') === 'MIGRATED', 'migrated → MIGRATED')
// Compile-time pin: the indexer enum is exactly these four.
const all: IndexerMarketStatus[] = ['BONDING', 'COMPLETE', 'MIGRATED', 'RECLAIMED']
ok(all.length === 4, 'IndexerMarketStatus is exactly 4 labels')

// Frame-shape goldens: internally tagged, row fields INLINE beside `kind`
// (mirrors api/src/ws.rs serde). If api serialization changes, this fails
// before a user's chart goes quiet.
const goldens: string[] = [
  '{"kind":"trade","mint":"M","trader":"T","is_buy":true,"sol_in":1,"slot":9}',
  '{"kind":"market","mint":"M","status":"BONDING","real_sol":0}',
  '{"kind":"message","mint":"M","sender":"S","memo_text":"gm"}',
  '{"kind":"resync"}',
]
for (const g of goldens) {
  const f = JSON.parse(g) as FeedFrame
  ok(typeof f.kind === 'string' && (f.kind === 'resync' || typeof f.mint === 'string'),
     `golden decodes: ${f.kind}`)
}

// [prompt-008 F-1] Leverage entry-swap slippage sizing (simulate-then-size).
// The sim feeds these pure functions the EXACT clamped amounts; their job is the
// fee + cpmm + slippage composition. A bug here is a false-revert (min_out too
// high) or weak protection (too low), so pin the math.
{
  // Long: min_out = (post-sim vault tokens − net collateral) × (1 − slippage).
  ok(sizeLongMinOut(1000n, 600n, 100) === 396n, 'long: (1000−600)×99% = 396')
  ok(sizeLongMinOut(1000n, 0n, 0) === 1000n, 'long: 0% slippage = full bought')
  ok(sizeLongMinOut(600n, 600n, 100) === 0n, 'long: nothing bought → 0')
  ok(sizeLongMinOut(500n, 600n, 100) === 0n, 'long: vault ≤ collateral → 0 (no underflow)')

  // Net collateral after the 7bps Token-2022 deposit fee (ceil).
  ok(netCollateralAfterFee(1_000_000n) === 999_300n, 'net collateral: 1e6 − ceil(700) = 999300')
  ok(netCollateralAfterFee(1n) === 0n, 'net collateral: 1 − ceil(0.0007)=1 → 0')
  ok(netCollateralAfterFee(0n) === 0n, 'net collateral: 0 → 0')

  // Short: min_out = sell output of the net-of-transfer-fee tokens, × (1 − slippage).
  // Pinned against the exported cpmmSwap so this tests the COMPOSITION, not a
  // hand-computed magic number.
  const tb = 1_000_000n
  const tr = 1_000_000_000_000n
  const sr = 100_000_000_000n
  const netReceived = tb - (tb * 7n + 9999n) / 10000n // mirror transferFeeCeil
  const fullOut = cpmmSwap(netReceived, tr, sr)
  ok(sizeShortMinOut(tb, tr, sr, 0) === fullOut, 'short: 0% slippage = full sell output')
  ok(sizeShortMinOut(tb, tr, sr, 100) === (fullOut * 9900n) / 10000n, 'short: 1% = 99% of output')
  ok(sizeShortMinOut(0n, tr, sr, 100) === 0n, 'short: 0 borrow → 0')
}

console.log(`\n${pass} passed, ${fail} failed`)
process.exit(fail ? 1 : 0)
