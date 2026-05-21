'use client'

import { createContext, useContext, useCallback, useSyncExternalStore, ReactNode } from 'react'

export type NetworkId = 'simnet' | 'devnet' | 'mainnet'

export interface NetworkConfig {
  id: NetworkId
  name: string
  rpcUrl: string
  wsUrl?: string
  explorerUrl: string
  jupiterEnabled: boolean
  isSimnet?: boolean
}

// Cloudflare Worker proxy for Helius RPC (keeps API key server-side)
const HELIUS_PROXY_URL = 'https://torch-market-rpc.mrsirg97.workers.dev'
const HELIUS_WS_URL = 'wss://torch-market-rpc.mrsirg97.workers.dev'

const NETWORKS: Record<NetworkId, NetworkConfig> = {
  simnet: {
    id: 'simnet',
    name: 'Simnet (Local Fork)',
    rpcUrl: 'http://localhost:8899',
    wsUrl: 'ws://localhost:8900',
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: false,
    isSimnet: true,
  },
  devnet: {
    id: 'devnet',
    name: 'Devnet',
    rpcUrl: `${HELIUS_PROXY_URL}/devnet`,
    wsUrl: `${HELIUS_WS_URL}/devnet`,
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: false,
  },
  mainnet: {
    id: 'mainnet',
    name: 'Mainnet',
    rpcUrl: HELIUS_PROXY_URL,
    wsUrl: HELIUS_WS_URL,
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: false,
  },
}

const STORAGE_KEY = 'torch-network'
const CUSTOM_RPC_KEY = 'torch-custom-rpc'

// Check if running on localhost (where simnet is available)
function isLocalhost(): boolean {
  if (typeof window === 'undefined') return false
  return window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1'
}

// Get default network from env or localStorage
function getDefaultNetworkId(): NetworkId {
  // Check localStorage first (client-side only)
  if (typeof window !== 'undefined') {
    const stored = localStorage.getItem(STORAGE_KEY)
    // Simnet only available on localhost
    if (stored === 'simnet' && !isLocalhost()) {
      localStorage.removeItem(STORAGE_KEY)
      return 'mainnet'
    }
    if (stored && (stored === 'simnet' || stored === 'devnet' || stored === 'mainnet')) {
      return stored
    }
  }
  // Fall back to env variable
  const envNetwork = process.env.NEXT_PUBLIC_NETWORK
  if (envNetwork === 'simnet' && isLocalhost()) {
    return envNetwork
  }
  if (envNetwork === 'devnet' || envNetwork === 'mainnet') {
    return envNetwork
  }
  // Default to mainnet for production
  return 'mainnet'
}

interface NetworkContextType {
  network: NetworkConfig
  networkId: NetworkId
  setNetworkId: (id: NetworkId) => void
  isMainnet: boolean
  isDevnet: boolean
  isSimnet: boolean
  /** Custom RPC URL set by the user (empty string = use default) */
  customRpcUrl: string
  /** Set a custom RPC URL (empty string to clear) */
  setCustomRpcUrl: (url: string) => void
  /** The actual RPC URL in use (custom if set, otherwise default for network) */
  effectiveRpcUrl: string
  /** The actual WebSocket URL in use (null if custom RPC is set — no WS for custom) */
  effectiveWsUrl: string | undefined
}

const NetworkContext = createContext<NetworkContextType | null>(null)

// Subscribe to storage events for cross-tab sync
function subscribeToStorage(callback: () => void) {
  window.addEventListener('storage', callback)
  return () => window.removeEventListener('storage', callback)
}

// Get current network from localStorage (client-side)
function getNetworkSnapshot(): NetworkId {
  return getDefaultNetworkId()
}

// Server-side snapshot (always mainnet)
function getServerSnapshot(): NetworkId {
  return 'mainnet'
}

// Get custom RPC URL from localStorage
function getCustomRpcSnapshot(): string {
  if (typeof window === 'undefined') return ''
  return localStorage.getItem(CUSTOM_RPC_KEY) || ''
}

function getCustomRpcServerSnapshot(): string {
  return ''
}

export function NetworkProvider({ children }: { children: ReactNode }) {
  const networkId = useSyncExternalStore(subscribeToStorage, getNetworkSnapshot, getServerSnapshot)
  const customRpcUrl = useSyncExternalStore(subscribeToStorage, getCustomRpcSnapshot, getCustomRpcServerSnapshot)

  const setNetworkId = useCallback((id: NetworkId) => {
    if (typeof window !== 'undefined') {
      localStorage.setItem(STORAGE_KEY, id)
      // Sync to globalThis so torchsdk picks up the network at runtime
      ;(globalThis as any).__TORCH_NETWORK__ = id === 'devnet' ? 'devnet' : ''
      // Dispatch event to trigger re-render in this tab
      window.dispatchEvent(new StorageEvent('storage', { key: STORAGE_KEY, newValue: id }))
    }
  }, [])

  const setCustomRpcUrl = useCallback((url: string) => {
    if (typeof window !== 'undefined') {
      if (url) {
        localStorage.setItem(CUSTOM_RPC_KEY, url)
      } else {
        localStorage.removeItem(CUSTOM_RPC_KEY)
      }
      window.dispatchEvent(new StorageEvent('storage', { key: CUSTOM_RPC_KEY, newValue: url || null }))
    }
  }, [])

  // Sync network to globalThis so torchsdk picks it up at runtime
  if (typeof window !== 'undefined') {
    ;(globalThis as any).__TORCH_NETWORK__ = networkId === 'devnet' ? 'devnet' : ''
  }

  const network = NETWORKS[networkId]

  // Custom RPC overrides the default for the current network
  const effectiveRpcUrl = customRpcUrl || network.rpcUrl
  // Disable WebSocket when using custom RPC (we don't know their WS endpoint)
  const effectiveWsUrl = customRpcUrl ? undefined : network.wsUrl

  const value: NetworkContextType = {
    network,
    networkId,
    setNetworkId,
    isMainnet: networkId === 'mainnet',
    isDevnet: networkId === 'devnet',
    isSimnet: networkId === 'simnet',
    customRpcUrl,
    setCustomRpcUrl,
    effectiveRpcUrl,
    effectiveWsUrl,
  }

  return <NetworkContext.Provider value={value}>{children}</NetworkContext.Provider>
}

export function useNetwork(): NetworkContextType {
  const context = useContext(NetworkContext)
  if (!context) {
    throw new Error('useNetwork must be used within a NetworkProvider')
  }
  return context
}

// For backward compatibility with existing code
export function getExplorerUrl(
  address: string,
  type: 'address' | 'tx' = 'address',
  networkId: NetworkId = 'mainnet',
): string {
  const network = NETWORKS[networkId]
  let cluster = ''
  if (network.id === 'simnet') {
    cluster = '?cluster=custom&customUrl=http://localhost:8899'
  } else if (network.id !== 'mainnet') {
    cluster = `?cluster=${network.id}`
  }
  return `${network.explorerUrl}/${type}/${address}${cluster}`
}
