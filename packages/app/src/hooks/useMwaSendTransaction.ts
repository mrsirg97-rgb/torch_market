'use client'

import { useCallback } from 'react'
import { useWallet } from '@solana/wallet-adapter-react'
import { useConnection } from '@solana/wallet-adapter-react'
import type { Transaction, VersionedTransaction, Connection } from '@solana/web3.js'

function isAndroidMobile() {
  return (
    typeof window !== 'undefined' &&
    window.isSecureContext &&
    typeof document !== 'undefined' &&
    /android/i.test(navigator.userAgent)
  )
}

/**
 * MWA-aware sendTransaction.
 *
 * On Android, wraps wallet.sendTransaction inside a transact() session
 * so the WebSocket is already open when sendTransaction fires.
 *
 * On desktop/iOS, uses wallet.sendTransaction directly.
 */
export function useMwaSendTransaction() {
  const wallet = useWallet()
  const { connection } = useConnection()

  const sendTransaction = useCallback(
    async (
      tx: Transaction | VersionedTransaction,
      conn?: Connection,
    ): Promise<string> => {
      const c = conn ?? connection

      if (!wallet.sendTransaction) {
        throw new Error('Wallet does not support sendTransaction')
      }

      if (isAndroidMobile()) {
        const { transact } = await import('@solana-mobile/mobile-wallet-adapter-protocol-web3js')
        return transact(async (wallet) => {
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
          const latestBlockhash = await connection.getLatestBlockhash()      
          const sig = await c.sendRawTransaction(txs[0].serialize())
          await connection.confirmTransaction({                                                                                                                                                               
            signature: sig,                                                                                                                                                                                   
            blockhash: latestBlockhash.blockhash,                   
            lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,                                                                                                                                       
          }, 'confirmed')                                                                                                                                             
          return sig
        })
      }

      return wallet.sendTransaction(tx, c)
    },
    [wallet, connection],
  )

  return sendTransaction
}
