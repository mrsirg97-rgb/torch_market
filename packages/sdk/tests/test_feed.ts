/**
 * TorchFeedClient state machine — pure unit, mock WebSocket. Pins the
 * contracts that break silently: refcounted rooms, resubscribe-on-reconnect
 * (Cloud Run cycles WS hourly), resync fan-out, mint-routed dispatch.
 * Run: npx tsx tests/test_feed.ts
 */
import { TorchFeedClient, FeedFrame } from '../src/feed'

class MockWS {
  static instances: MockWS[] = []
  static OPEN = 1
  readyState = 0
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) { MockWS.instances.push(this) }
  send(s: string) { this.sent.push(s) }
  close() { this.readyState = 3; this.onclose?.() }
  open() { this.readyState = 1; this.onopen?.() }
  frame(f: object) { this.onmessage?.({ data: JSON.stringify(f) }) }
}
;(globalThis as any).WebSocket = MockWS

let pass = 0, fail = 0
const ok = (cond: boolean, name: string) => {
  if (cond) { pass++; console.log(`ok   - ${name}`) }
  else { fail++; console.log(`FAIL - ${name}`) }
}

// ── subscribe sends protocol frame after open ──
{
  MockWS.instances = []
  const c = new TorchFeedClient('http://x')
  const got: FeedFrame[] = []
  c.subscribe({ market: 'M1' }, (f) => got.push(f))
  const ws = MockWS.instances[0]
  ws.open()
  ok(ws.sent.some((s) => s === '{"subscribe":{"market":"M1"}}'), 'subscribe frame sent on open')

  // ── mint-routed dispatch + 'all' coexistence ──
  const gotAll: FeedFrame[] = []
  c.subscribe('all', (f) => gotAll.push(f))
  ok(ws.sent.some((s) => s === '{"subscribe":"all"}'), 'all-room subscribe sent live')
  ws.frame({ kind: 'trade', mint: 'M1' })
  ws.frame({ kind: 'trade', mint: 'OTHER' })
  ok(got.filter((f) => f.kind === 'trade').length === 1, 'market room sees only its mint')
  ok(gotAll.filter((f) => f.kind === 'trade').length === 2, 'all room sees every tick')

  // ── server resync reaches every room ──
  ws.frame({ kind: 'resync' })
  ok(got.some((f) => f.kind === 'resync') && gotAll.some((f) => f.kind === 'resync'),
     'resync fans out to all rooms')

  // ── reconnect: resubscribe + synthetic resync ──
  const before = got.length
  ws.close()
  const ws2 = MockWS.instances[1]
  ok(!!ws2 === false, 'reconnect waits for backoff (no instant socket)')
  // backoff is 500ms; simulate by waiting
}
setTimeout(() => {
  const ws2 = MockWS.instances[1]
  ok(!!ws2, 'reconnect creates a new socket after backoff')
  if (ws2) {
    ws2.open()
    ok(ws2.sent.some((s) => s.includes('"market":"M1"')) && ws2.sent.some((s) => s === '{"subscribe":"all"}'),
       'reconnect resubscribes every active room')
  }
  // ── refcount: last unsubscribe sends protocol unsubscribe ──
  const c2 = new TorchFeedClient('http://y')
  const off1 = c2.subscribe({ market: 'Z' }, () => {})
  const off2 = c2.subscribe({ market: 'Z' }, () => {})
  const wsz = MockWS.instances[MockWS.instances.length - 1]
  wsz.open()
  off1()
  ok(!wsz.sent.some((s) => s.includes('unsubscribe')), 'room held while one handler remains')
  off2()
  ok(wsz.sent.some((s) => s.includes('unsubscribe')), 'last handler out → unsubscribe sent')

  console.log(`\n${pass} passed, ${fail} failed`)
  process.exit(fail ? 1 : 0)
}, 700)
