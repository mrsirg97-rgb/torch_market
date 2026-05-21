'use client'

import { useMemo, useSyncExternalStore } from 'react'
import { ConnectionProvider, WalletProvider } from '@solana/wallet-adapter-react'
import { WalletModalProvider } from '@solana/wallet-adapter-react-ui'
import { NetworkProvider, useNetwork } from '@/lib/NetworkContext'
import { ThemeProvider } from '@/lib/ThemeContext'
import {
  createDefaultAuthorizationCache,
  createDefaultChainSelector,
  createDefaultWalletNotFoundHandler,
  registerMwa,
} from '@solana-mobile/wallet-standard-mobile'

// Import wallet adapter CSS
import '@solana/wallet-adapter-react-ui/styles.css'

// Get URI for app identity (returns undefined during SSR)
function getUriForAppIdentity() {
  const location = globalThis.location
  if (!location) return undefined
  return `${location.protocol}//${location.host}`
}

// Check if running on Android mobile in secure context
function isAndroidMobile() {
  return (
    typeof window !== 'undefined' &&
    window.isSecureContext &&
    typeof document !== 'undefined' &&
    /android/i.test(navigator.userAgent)
  )
}

// Determine active chain from localStorage (matches NetworkContext)
function getActiveChain(): 'solana:mainnet' | 'solana:devnet' {
  if (typeof window === 'undefined') return 'solana:mainnet'
  const stored = localStorage.getItem('torch-network')
  return stored === 'devnet' ? 'solana:devnet' : 'solana:mainnet'
}

// Register Mobile Wallet Adapter only on Android mobile devices
// On desktop, this pollutes the Wallet Standard registry and can
// shadow or race with real browser-extension wallets (Phantom, etc.)
if (isAndroidMobile()) {
  registerMwa({
    appIdentity: {
      name: 'Torch Market',
      uri: getUriForAppIdentity(),
      icon: '/apple-touch-icon.png',
    },
    authorizationCache: createDefaultAuthorizationCache(),
    chains: [getActiveChain()],
    chainSelector: createDefaultChainSelector(),
    onWalletNotFound: createDefaultWalletNotFoundHandler(),
  })
}

function SolanaProviders({ children }: { children: React.ReactNode }) {
  const { networkId, isSimnet, effectiveRpcUrl, effectiveWsUrl, customRpcUrl } = useNetwork()

  const { endpoint, wsEndpoint } = useMemo(() => {
    return {
      endpoint: effectiveRpcUrl,
      // Disable WebSocket for simnet (surfpool doesn't support programSubscribe)
      // and when custom RPC is set (we don't know their WS endpoint)
      wsEndpoint: isSimnet ? undefined : effectiveWsUrl,
    }
  }, [effectiveRpcUrl, effectiveWsUrl, isSimnet])

  const config = useMemo(
    () => ({
      commitment: 'confirmed' as const,
      ...(wsEndpoint ? { wsEndpoint } : {}),
    }),
    [wsEndpoint],
  )

  // Empty array = use Wallet Standard auto-detection
  // registerMwa above adds MWA to the Wallet Standard registry
  const wallets = useMemo(() => [], [])

  // Key forces remount when network or custom RPC changes, reinitializing the connection
  // autoConnect only on Android mobile to trigger MWA automatically
  return (
    <ConnectionProvider key={`${networkId}-${customRpcUrl}`} endpoint={endpoint} config={config}>
      <WalletProvider wallets={wallets} autoConnect={isAndroidMobile()}>
        <WalletModalProvider>{children}</WalletModalProvider>
      </WalletProvider>
    </ConnectionProvider>
  )
}

// useSyncExternalStore for hydration-safe client detection
const emptySubscribe = () => () => {}
const getClientSnapshot = () => true
const getServerSnapshot = () => false

export function Providers({ children }: { children: React.ReactNode }) {
  const mounted = useSyncExternalStore(emptySubscribe, getClientSnapshot, getServerSnapshot)

  // Don't render until client-side mounted
  if (!mounted) {
    return null
  }

  return (
    <ThemeProvider>
      <NetworkProvider>
        <SolanaProviders>{children}</SolanaProviders>
      </NetworkProvider>
    </ThemeProvider>
  )
}
