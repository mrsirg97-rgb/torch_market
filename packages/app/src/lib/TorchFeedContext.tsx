'use client'

// One multiplexed WS connection for the whole app (prompt-003 rooms).
// Components subscribe to rooms via useTorchFeed; the SDK client handles
// reconnect + resubscribe + Resync. Polling hooks stay alive as the RPC
// fallback — demoted to slow heartbeat while the feed is open.

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  ReactNode,
} from 'react'
import { TorchFeedClient, FeedFrame, FeedStatus, RoomTarget } from 'torchsdk'
import { useNetwork } from '@/lib/NetworkContext'

interface TorchFeedContextType {
  client: TorchFeedClient | null
  status: FeedStatus
}

const TorchFeedContext = createContext<TorchFeedContextType>({
  client: null,
  status: 'closed',
})

export function TorchFeedProvider({ children }: { children: ReactNode }) {
  const { effectiveIndexerUrl } = useNetwork()
  const [status, setStatus] = useState<FeedStatus>('closed')

  const client = useMemo(
    () => (effectiveIndexerUrl ? new TorchFeedClient(effectiveIndexerUrl) : null),
    [effectiveIndexerUrl],
  )

  useEffect(() => {
    if (!client) return
    const off = client.onStatus(setStatus)
    return () => {
      off()
      client.close()
    }
  }, [client])

  return (
    <TorchFeedContext.Provider value={{ client, status }}>
      {children}
    </TorchFeedContext.Provider>
  )
}

/**
 * Subscribe to a room for the lifetime of the component. `onFrame` fires for
 * every frame in the room INCLUDING `resync` — treat resync as "refetch what
 * you render". Pass `null` target to disable (e.g. while mint is unknown).
 */
export function useTorchFeed(
  target: RoomTarget | null,
  onFrame: (frame: FeedFrame) => void,
): FeedStatus {
  const { client, status } = useContext(TorchFeedContext)
  const handlerRef = useRef(onFrame)
  handlerRef.current = onFrame

  const targetKey = target === null ? null : target === 'all' ? 'all' : target.market

  useEffect(() => {
    if (!client || targetKey === null) return
    const t: RoomTarget = targetKey === 'all' ? 'all' : { market: targetKey }
    return client.subscribe(t, (f) => handlerRef.current(f))
  }, [client, targetKey])

  return status
}

/** True while the live feed is open — poll loops use this to slow down. */
export function useFeedHealthy(): boolean {
  return useContext(TorchFeedContext).status === 'open'
}
