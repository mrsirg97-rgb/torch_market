/**
 * Create and populate an Address Lookup Table (ALT) for the Torch SDK.
 *
 * ALTs compress repeated account addresses from 32 bytes to 1 byte per
 * transaction, giving VersionedTransactions more room for instructions.
 *
 * V20: includes DeepPool program ID + the static DeepPool/Torch namespace PDAs
 * that recur across migration, vault swaps, and treasury fee swaps. No WSOL
 * mint (DeepPool uses native SOL) and no Raydium addresses (Raydium dependency
 * removed in V20).
 *
 * Usage:
 *   TORCH_NETWORK=devnet  KEYPAIR=~/.config/solana/id.json                              npx tsx scripts/create-alt.ts
 *   TORCH_NETWORK=mainnet KEYPAIR=~/Projects/torch_market/keys/mainnet-deploy-wallet.json npx tsx scripts/create-alt.ts
 *
 * After running, set the output address as one of:
 *   TORCH_ALT_MAINNET=<address>
 *   TORCH_ALT_DEVNET=<address>
 */

import {
  Connection,
  Keypair,
  PublicKey,
  AddressLookupTableProgram,
  TransactionMessage,
  VersionedTransaction,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
} from '@solana/web3.js'
import {
  PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  MEMO_PROGRAM_ID,
  DEEP_POOL_PROGRAM_ID,
} from '../src/constants'
import {
  getGlobalConfigPda,
  getProtocolTreasuryPda,
  getTorchConfigPda,
  getDeepPoolEventAuthorityPda,
} from '../src/program'
import { TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID } from '@solana/spl-token'
import fs from 'fs'
import path from 'path'

const RPC_URL =
  process.env.RPC_URL ??
  (process.env.TORCH_NETWORK === 'devnet'
    ? 'https://api.devnet.solana.com'
    : 'https://api.mainnet-beta.solana.com')

const loadKeypair = (): Keypair => {
  const keypairPath =
    process.env.KEYPAIR ?? path.join(process.env.HOME ?? '~', '.config', 'solana', 'id.json')
  const raw = JSON.parse(fs.readFileSync(keypairPath, 'utf-8'))
  return Keypair.fromSecretKey(Uint8Array.from(raw))
}

const confirmTx = async (connection: Connection, signature: string) => {
  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash()
  await connection.confirmTransaction(
    { signature, blockhash, lastValidBlockHeight },
    'confirmed',
  )
}

const main = async () => {
  const connection = new Connection(RPC_URL, 'confirmed')
  const payer = loadKeypair()
  const network = process.env.TORCH_NETWORK === 'devnet' ? 'devnet' : 'mainnet'

  console.log(`Network:  ${network}`)
  console.log(`RPC:      ${RPC_URL}`)
  console.log(`Payer:    ${payer.publicKey.toBase58()}`)

  // Static singleton PDAs — same for every transaction
  const [globalConfigPda] = getGlobalConfigPda()
  const [protocolTreasuryPda] = getProtocolTreasuryPda()
  const [torchConfigPda] = getTorchConfigPda()
  const [deepPoolEventAuthority] = getDeepPoolEventAuthorityPda()

  const addresses: PublicKey[] = [
    // Core program IDs
    PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID,
    TOKEN_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID,
    SystemProgram.programId,
    SYSVAR_RENT_PUBKEY,
    MEMO_PROGRAM_ID,
    DEEP_POOL_PROGRAM_ID,

    // Static PDAs (same across all tokens)
    globalConfigPda,
    protocolTreasuryPda,
    torchConfigPda, // namespace PDA torch_market signs as for every DeepPool create_pool
    deepPoolEventAuthority, // DeepPool's #[event_cpi] authority, required on every DeepPool ix
  ]

  console.log(`\nAddresses to include (${addresses.length}):`)
  addresses.forEach((addr, i) => console.log(`  ${i + 1}. ${addr.toBase58()}`))

  // Step 1: Create the ALT
  const slot = await connection.getSlot('finalized')

  const [createIx, altAddress] = AddressLookupTableProgram.createLookupTable({
    authority: payer.publicKey,
    payer: payer.publicKey,
    recentSlot: slot,
  })

  console.log(`\nCreating ALT: ${altAddress.toBase58()}`)

  const { blockhash } = await connection.getLatestBlockhash()
  const createMsg = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [createIx],
  }).compileToV0Message()

  const createTx = new VersionedTransaction(createMsg)
  createTx.sign([payer])
  const createSig = await connection.sendTransaction(createTx)
  await confirmTx(connection, createSig)
  console.log(`  Created: ${createSig}`)

  // Step 2: Extend the ALT with addresses (max 30 per tx)
  const BATCH_SIZE = 30
  for (let i = 0; i < addresses.length; i += BATCH_SIZE) {
    const batch = addresses.slice(i, i + BATCH_SIZE)

    const extendIx = AddressLookupTableProgram.extendLookupTable({
      payer: payer.publicKey,
      authority: payer.publicKey,
      lookupTable: altAddress,
      addresses: batch,
    })

    const { blockhash: extBlockhash } = await connection.getLatestBlockhash()
    const extMsg = new TransactionMessage({
      payerKey: payer.publicKey,
      recentBlockhash: extBlockhash,
      instructions: [extendIx],
    }).compileToV0Message()

    const extTx = new VersionedTransaction(extMsg)
    extTx.sign([payer])
    const extSig = await connection.sendTransaction(extTx)
    await confirmTx(connection, extSig)
    console.log(`  Extended batch ${Math.floor(i / BATCH_SIZE) + 1}: ${extSig}`)
  }

  const envVar = network === 'devnet' ? 'TORCH_ALT_DEVNET' : 'TORCH_ALT_MAINNET'
  console.log(`\nDone! Set the following environment variable:`)
  console.log(`  ${envVar}=${altAddress.toBase58()}`)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
