'use client'

import { useCallback } from 'react'
import { useWallet } from '@solana/wallet-adapter-react'
import { useConnection } from '@solana/wallet-adapter-react'
import type { Transaction, VersionedTransaction, Connection } from '@solana/web3.js'
import { useNetwork, type NetworkId } from '@/lib/NetworkContext'

function isAndroidMobile() {
  return (
    typeof window !== 'undefined' &&
    window.isSecureContext &&
    typeof document !== 'undefined' &&
    /android/i.test(navigator.userAgent)
  )
}

// Known cluster genesis hashes for the pre-flight sanity check. If our
// connection's genesis hash doesn't match the network we *think* we're on,
// something is wrong with the RPC config — bail before signing.
const GENESIS_BY_NETWORK: Record<NetworkId, string | null> = {
  mainnet: '5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d',
  devnet: 'EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG',
  simnet: null, // local validator has a unique genesis per spin-up; can't check
}

const TX_TIMEOUT_MS = 60_000

// Genesis hash never changes for a given RPC endpoint. Cache per URL so
// only the first send pays the round-trip; subsequent sends are zero-cost.
const genesisCache = new Map<string, string>()

class NetworkMismatchError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'NetworkMismatchError'
  }
}

function withTimeout<T>(p: Promise<T>, ms: number, msg: string): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new NetworkMismatchError(msg)), ms),
    ),
  ])
}

/**
 * MWA-aware sendTransaction with cluster-mismatch protection.
 *
 * Two layers of defense against the "wallet on mainnet, app on devnet"
 * (or vice-versa) hang:
 *
 * 1. **Pre-flight genesis check** — before we ever ask the wallet to sign,
 *    fetch the connection's genesis hash and compare against the known
 *    genesis for the app's selected network. If they disagree, throw a
 *    NetworkMismatchError immediately. This catches misconfigured RPC URLs
 *    AND the common case where the user has the wallet on a different
 *    cluster than the app (because most well-behaved wallets refuse to
 *    sign txs whose blockhash isn't from their selected cluster — but
 *    they signal this via a hang on signAndSendTransaction rather than
 *    a clean rejection).
 *
 * 2. **Timeout wrapper** — if the wallet still hangs (silent refusal,
 *    flaky connection, broadcast routed to a different cluster), reject in
 *    60s with an actionable error naming the expected network.
 *
 * We deliberately keep `wallet.sendTransaction` as the signing primitive
 * instead of `signTransaction` + `sendRawTransaction`. The latter pattern
 * is what drainer scams use (sign opaquely, broadcast wherever) and
 * Phantom rightfully flags it as malicious. `sendTransaction` is the
 * idiomatic, security-reviewed wallet flow.
 */
export function useMwaSendTransaction() {
  const wallet = useWallet()
  const { connection } = useConnection()
  const { networkId, network } = useNetwork()

  const sendTransaction = useCallback(
    async (
      tx: Transaction | VersionedTransaction,
      conn?: Connection,
    ): Promise<string> => {
      const c = conn ?? connection

      if (!wallet.publicKey) {
        throw new Error('Wallet not connected')
      }
      if (!wallet.sendTransaction) {
        throw new Error('Wallet does not support sendTransaction')
      }

      // Pre-flight genesis check. The connection's genesis hash must match
      // the cluster the app thinks it's on. Skipped for simnet (no
      // well-known genesis) and silently skipped if getGenesisHash itself
      // throws (treat as best-effort; tx broadcast will surface real errors).
      // Result is cached per endpoint — genesis is immutable for a given URL.
      const expected = GENESIS_BY_NETWORK[networkId]
      if (expected) {
        let genesis = genesisCache.get(c.rpcEndpoint) ?? null
        if (!genesis) {
          try {
            genesis = await c.getGenesisHash()
            genesisCache.set(c.rpcEndpoint, genesis)
          } catch {
            // RPC unavailable — fall through, the send call will fail loudly
          }
        }
        if (genesis && genesis !== expected) {
          throw new NetworkMismatchError(
            `Connection points to a different Solana cluster than the app's selected network (${network.name}). ` +
              `If your wallet is on a different cluster than Torch, switch it to ${network.name} and try again.`,
          )
        }
      }

      if (isAndroidMobile()) {
        const { transact } = await import('@solana-mobile/mobile-wallet-adapter-protocol-web3js')
        return withTimeout(
          transact(async (wallet) => {
            const cached = localStorage.getItem('mwa-auth-token')
            if (cached) {
              await wallet.reauthorize({
                auth_token: cached, identity: {
                  name: 'Torch Market',
                  uri: 'https://torch.market',
                  icon: '/apple-touch-icon.png',
                }
              })
            } else {
              const auth = await wallet.authorize({
                identity: {
                  name: 'Torch Market',
                  uri: 'https://torch.market',
                  icon: '/apple-touch-icon.png',
                }
              })
              localStorage.setItem('mwa-auth-token', auth.auth_token)
            }

            const txs = await wallet.signTransactions({ transactions: [tx] })
            const latestBlockhash = await c.getLatestBlockhash()
            const sig = await c.sendRawTransaction(txs[0].serialize())
            await c.confirmTransaction({
              signature: sig,
              blockhash: latestBlockhash.blockhash,
              lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,
            }, 'confirmed')
            return sig
          }),
          TX_TIMEOUT_MS,
          `Transaction timed out after ${TX_TIMEOUT_MS / 1000}s. Check your wallet is set to ${network.name}.`,
        )
      }

      return withTimeout(
        wallet.sendTransaction(tx, c),
        TX_TIMEOUT_MS,
        `Transaction timed out after ${TX_TIMEOUT_MS / 1000}s. Check your wallet is set to ${network.name}.`,
      )
    },
    [wallet, connection, networkId, network],
  )

  return sendTransaction
}
