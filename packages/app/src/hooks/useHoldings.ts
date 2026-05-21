'use client'

import { useEffect, useState } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token'
import { getTorchVaultPda } from 'torchsdk'
import { TokenData } from '@/types/token'

const isDev = process.env.NODE_ENV === 'development'

export interface HoldingEntry {
  /** Balance held directly in the connected wallet (Token-2022 ATA). */
  wallet: bigint
  /** Balance held by the wallet's torch vault PDA, if any. 0 if no vault. */
  vault: bigint
}

/**
 * Aggregated Token-2022 balances for `tokens` across the connected wallet
 * AND its torch vault PDA. Only mints with a non-zero total appear in the map.
 *
 * Both wallet ATAs and vault ATAs are batched into a single
 * getMultipleAccountsInfo call. The vault PDA is derived deterministically
 * from the wallet pubkey — no roundtrip needed to know its address — and ATAs
 * that don't exist on chain simply return null and contribute 0.
 *
 * Returns an empty map when no wallet is connected.
 */
export function useHoldings(tokens: TokenData[]): Map<string, HoldingEntry> {
  const { connection } = useConnection()
  const { publicKey } = useWallet()
  const [balances, setBalances] = useState<Map<string, HoldingEntry>>(new Map())

  useEffect(() => {
    let cancelled = false

    const run = async () => {
      await Promise.resolve()
      if (cancelled) return

      if (!publicKey || tokens.length === 0) {
        setBalances(new Map())
        return
      }

      try {
        const [vaultPda] = getTorchVaultPda(publicKey)

        // Build (mint, walletAta, vaultAta) triples, then flatten into one
        // batch keyed by index so we can fan back out by mint.
        const entries = tokens.map((t) => {
          const mint = new PublicKey(t.mint)
          return {
            mint: t.mint,
            walletAta: getAssociatedTokenAddressSync(
              mint,
              publicKey,
              false,
              TOKEN_2022_PROGRAM_ID,
            ),
            vaultAta: getAssociatedTokenAddressSync(
              mint,
              vaultPda,
              true, // allowOwnerOffCurve — vault is a PDA
              TOKEN_2022_PROGRAM_ID,
            ),
          }
        })

        const atasFlat: PublicKey[] = []
        for (const e of entries) {
          atasFlat.push(e.walletAta, e.vaultAta)
        }

        const infos = await connection.getMultipleAccountsInfo(atasFlat)
        if (cancelled) return

        const readBalance = (info: (typeof infos)[number]): bigint =>
          info && info.data.length >= 72 ? info.data.readBigUInt64LE(64) : BigInt(0)

        const map = new Map<string, HoldingEntry>()
        entries.forEach((e, i) => {
          const wallet = readBalance(infos[i * 2])
          const vault = readBalance(infos[i * 2 + 1])
          if (wallet > BigInt(0) || vault > BigInt(0)) {
            map.set(e.mint, { wallet, vault })
          }
        })
        setBalances(map)
      } catch (err) {
        if (isDev) console.error('useHoldings: failed to fetch balances', err)
        if (!cancelled) setBalances(new Map())
      }
    }

    run()
    return () => {
      cancelled = true
    }
  }, [publicKey, connection, tokens])

  return balances
}

/** Convenience: total balance (wallet + vault) for a mint, or 0n. */
export function totalBalance(map: Map<string, HoldingEntry>, mint: string): bigint {
  const e = map.get(mint)
  return e ? e.wallet + e.vault : BigInt(0)
}
