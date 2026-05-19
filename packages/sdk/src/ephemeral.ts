/*
 * Ephemeral Agent Keypair
 *
 * generates an in-process keypair that lives only in memory.
 * the authority links this wallet to their vault, and the SDK uses it to sign transactions.
 * when the process stops, the private key is lost — zero key management, zero risk.
 * flow:
 *   1. const agent = createEphemeralAgent()
 *   2. Authority calls buildLinkWalletTransaction({ wallet_to_link: agent.publicKey })
 *   3. SDK uses agent.sign(tx) for all vault operations
 *   4. On shutdown, keys are GC'd. Authority unlinks the wallet.
 */
import { Keypair, Transaction, VersionedTransaction } from '@solana/web3.js'

export interface EphemeralAgent {
  publicKey: string // base58 public key — pass this to linkWallet
  keypair: Keypair // raw keypair for advanced usage
  sign(tx: Transaction | VersionedTransaction): Transaction | VersionedTransaction // sign a transaction with the ephemeral key
}

// create an ephemeral agent keypair.
// the keypair exists only in memory. No file is written to disk.
// when the process exits, the private key is permanently lost.
export const createEphemeralAgent = (): EphemeralAgent => {
  const keypair = Keypair.generate()
  return {
    publicKey: keypair.publicKey.toBase58(),
    keypair,
    sign: (tx: Transaction | VersionedTransaction) => {
      if (tx instanceof VersionedTransaction) {
        tx.sign([keypair])
      } else {
        tx.partialSign(keypair)
      }
      return tx
    },
  }
}
