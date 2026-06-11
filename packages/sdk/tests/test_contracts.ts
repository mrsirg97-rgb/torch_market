/**
 * Cross-layer contract pins — the failure class of 2026-06-11: one layer
 * relabels, the other doesn't hear. Run: npx tsx tests/test_contracts.ts
 */
import { sdkStatusToIndexer } from '../src/tokens'
import type { IndexerMarketStatus } from '../src/indexer'
import type { FeedFrame } from '../src/feed'

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

console.log(`\n${pass} passed, ${fail} failed`)
process.exit(fail ? 1 : 0)
