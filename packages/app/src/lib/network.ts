import { clusterApiUrl } from '@solana/web3.js'

export type NetworkId = 'local' | 'devnet' | 'mainnet'

export interface NetworkConfig {
  id: NetworkId
  name: string
  rpcUrl: string
  explorerUrl: string
  // Jupiter only works on mainnet
  jupiterEnabled: boolean
}

// Cloudflare Worker proxy for Helius RPC (keeps API key server-side)
const HELIUS_PROXY_URL = 'https://torch-market-rpc.mrsirg97.workers.dev'

const NETWORKS: Record<NetworkId, NetworkConfig> = {
  local: {
    id: 'local',
    name: 'Localhost',
    rpcUrl: 'http://localhost:8899',
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: false,
  },
  devnet: {
    id: 'devnet',
    name: 'Devnet',
    rpcUrl: clusterApiUrl('devnet'),
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: false,
  },
  mainnet: {
    id: 'mainnet',
    name: 'Mainnet',
    // Client uses Helius proxy to keep API key server-side
    rpcUrl: HELIUS_PROXY_URL,
    explorerUrl: 'https://explorer.solana.com',
    jupiterEnabled: true,
  },
}

const STORAGE_KEY = 'torch-network'

// Get network from localStorage or environment variable
function getNetworkId(): NetworkId {
  // Check localStorage first (client-side only)
  if (typeof window !== 'undefined') {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored && NETWORKS[stored as NetworkId]) {
      return stored as NetworkId
    }
  }
  // Fall back to env variable
  const envNetwork = process.env.NEXT_PUBLIC_NETWORK as NetworkId
  if (envNetwork && NETWORKS[envNetwork]) {
    return envNetwork
  }
  // Default to mainnet
  return 'mainnet'
}

// Allow RPC URL override via environment variable
function getRpcUrl(network: NetworkConfig): string {
  const envRpcUrl = process.env.NEXT_PUBLIC_RPC_URL
  if (envRpcUrl) {
    return envRpcUrl
  }
  return network.rpcUrl
}

export function getNetwork(): NetworkConfig {
  const networkId = getNetworkId()
  const network = NETWORKS[networkId]
  return {
    ...network,
    rpcUrl: getRpcUrl(network),
  }
}

export function getExplorerUrl(address: string, type: 'address' | 'tx' = 'address'): string {
  const network = getNetwork()
  const cluster =
    network.id === 'mainnet'
      ? ''
      : `?cluster=${network.id === 'local' ? 'custom&customUrl=http://localhost:8899' : network.id}`
  return `${network.explorerUrl}/${type}/${address}${cluster}`
}

export function isMainnet(): boolean {
  return getNetwork().id === 'mainnet'
}

export function isDevnet(): boolean {
  return getNetwork().id === 'devnet'
}

export function isLocalnet(): boolean {
  return getNetwork().id === 'local'
}
