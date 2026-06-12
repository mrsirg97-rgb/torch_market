/**
 * TorchFeedClient — typed client for the read-API's WS rooms (prompt-003).
 *
 * The api fans events out per ROOM, not as a firehose:
 *   subscribe({ market: mint })  — everything for one market
 *   subscribe('all')             — index-page heartbeat (market rows + trade/swap ticks)
 *
 * Frames are internally tagged: { kind: 'trade', ...TradeRow }. Two contracts
 * consumers MUST honor:
 *   - 'resync': the api had a LISTEN gap (or you lagged) — refetch what you
 *     render. Treat refetch as cheap; rows are snapshots.
 *   - reconnect: server-side room membership dies with the socket. The client
 *     resubscribes every active room automatically and emits a synthetic
 *     'resync' so consumers refetch across the gap.
 *
 * Framework-agnostic (browser WebSocket); the app wraps this in a provider.
 */

export type RoomTarget = 'all' | { market: string }

export type FeedFrameKind =
  | 'pool'
  | 'swap'
  | 'liquidity'
  | 'reserves'
  | 'market'
  | 'trade'
  | 'message'
  | 'position'
  | 'position_event'
  | 'migration'
  | 'resync'

/** Internally tagged: row fields are inlined beside `kind`. */
export interface FeedFrame {
  kind: FeedFrameKind
  mint?: string
  pool_id?: number
  [field: string]: unknown
}

export type FeedHandler = (frame: FeedFrame) => void
export type FeedStatus = 'connecting' | 'open' | 'closed'

const roomKey = (t: RoomTarget): string => (t === 'all' ? 'all' : `market:${t.market}`)

const BACKOFF_MS = [500, 1000, 2000, 5000, 10000]

export class TorchFeedClient {
  private url: string
  private ws: WebSocket | null = null
  private handlers = new Map<string, Set<FeedHandler>>() // roomKey → handlers
  private attempts = 0
  private closedByUser = false
  private statusListeners = new Set<(s: FeedStatus) => void>()

  constructor(indexerUrl: string) {
    this.url = indexerUrl.replace(/^http/, 'ws') + '/events'
  }

  /** Subscribe a handler to a room. Returns an unsubscribe fn. Connects lazily. */
  subscribe(target: RoomTarget, handler: FeedHandler): () => void {
    // A new subscriber revives a closed client (React StrictMode double-runs
    // provider effects in dev: close() then resubscribe on the same instance).
    this.closedByUser = false
    const key = roomKey(target)
    let set = this.handlers.get(key)
    const isNewRoom = !set
    if (!set) {
      set = new Set()
      this.handlers.set(key, set)
    }
    set.add(handler)

    if (!this.ws) {
      this.connect()
    } else if (isNewRoom && this.ws.readyState === WebSocket.OPEN) {
      this.send(target)
    }

    return () => {
      const s = this.handlers.get(key)
      if (!s) return
      s.delete(handler)
      if (s.size === 0) {
        this.handlers.delete(key)
        if (this.ws?.readyState === WebSocket.OPEN) {
          this.ws.send(JSON.stringify({ unsubscribe: target === 'all' ? 'all' : target }))
        }
      }
    }
  }

  onStatus(fn: (s: FeedStatus) => void): () => void {
    this.statusListeners.add(fn)
    return () => this.statusListeners.delete(fn)
  }

  close(): void {
    this.closedByUser = true
    const ws = this.ws
    if (ws) {
      if (ws.readyState === WebSocket.CONNECTING) {
        // Closing a CONNECTING socket makes browsers log a warning (React
        // StrictMode double-mounts hit this constantly in dev). Detach
        // handlers and close once the handshake settles instead.
        ws.onmessage = null
        ws.onclose = null
        ws.onerror = null
        ws.onopen = () => ws.close()
      } else {
        ws.close()
      }
    }
    this.ws = null
  }

  private send(target: RoomTarget): void {
    this.ws?.send(JSON.stringify({ subscribe: target === 'all' ? 'all' : target }))
  }

  private emitStatus(s: FeedStatus): void {
    for (const fn of this.statusListeners) fn(s)
  }

  private connect(): void {
    if (this.closedByUser) return
    this.emitStatus('connecting')
    const ws = new WebSocket(this.url)
    this.ws = ws

    ws.onopen = () => {
      this.attempts = 0
      this.emitStatus('open')
      // Re-join every active room — server membership died with the old
      // socket — then tell consumers to refetch across the gap.
      for (const key of this.handlers.keys()) {
        this.send(key === 'all' ? 'all' : { market: key.slice('market:'.length) })
      }
      this.dispatchAll({ kind: 'resync' })
    }

    ws.onmessage = (e: MessageEvent) => {
      try {
        const frame = JSON.parse(e.data as string) as FeedFrame
        if (frame.kind === 'resync') {
          // Server-side gap: every room refetches.
          this.dispatchAll(frame)
          return
        }
        this.dispatch(frame)
      } catch {
        /* malformed frame — ignore */
      }
    }

    ws.onclose = () => {
      this.ws = null
      this.emitStatus('closed')
      if (this.closedByUser || this.handlers.size === 0) return
      const delay = BACKOFF_MS[Math.min(this.attempts, BACKOFF_MS.length - 1)]
      this.attempts += 1
      setTimeout(() => this.connect(), delay)
    }
    ws.onerror = () => ws.close()
  }

  /** Route a typed frame to the rooms it belongs to. The server already
   *  scoped delivery per-connection-room, but one socket carries ALL our
   *  rooms — so route locally by frame content (mint). Kinds without a mint
   *  (none today besides resync) go everywhere. */
  private dispatch(frame: FeedFrame): void {
    const mint = typeof frame.mint === 'string' ? frame.mint : undefined
    const marketSet = mint ? this.handlers.get(`market:${mint}`) : undefined
    if (marketSet) for (const h of marketSet) h(frame)
    const allSet = this.handlers.get('all')
    if (allSet) for (const h of allSet) h(frame)
    if (!marketSet && !mint) {
      // No mint on the frame (e.g. pool-keyed rows before enrichment):
      // deliver to every market room rather than dropping.
      for (const [key, set] of this.handlers) {
        if (key !== 'all') for (const h of set) h(frame)
      }
    }
  }

  private dispatchAll(frame: FeedFrame): void {
    for (const set of this.handlers.values()) for (const h of set) h(frame)
  }
}
