/**
 * One-off bootstrap: initialize torch_market global_config.
 * Run against surfpool (or any fresh deploy) before any other e2e.
 *
 *   npx tsx scripts/bootstrap_global_config.ts
 *
 * Env:
 *   RPC_URL       (default http://127.0.0.1:8899)
 *   PAYER_KEYPAIR (default ~/.config/solana/id.json)
 */

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from '@solana/web3.js'
import { BorshCoder, Idl } from '@coral-xyz/anchor'
import * as fs from 'fs'
import * as os from 'os'
import * as path from 'path'

import { PROGRAM_ID } from '../src/constants'
import { getGlobalConfigPda, getProtocolTreasuryPda } from '../src/program'
import idl from '../src/torch_market.json'

async function main() {
  const rpc = process.env.RPC_URL || 'http://127.0.0.1:8899'
  const keypairPath =
    process.env.PAYER_KEYPAIR || path.join(os.homedir(), '.config/solana/id.json')

  const payer = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(keypairPath, 'utf8'))),
  )
  const connection = new Connection(rpc, 'confirmed')
  const coder = new BorshCoder(idl as unknown as Idl)

  // 1. initialize global_config
  const [globalConfig] = getGlobalConfigPda()
  if (await connection.getAccountInfo(globalConfig)) {
    console.log(`global_config already initialized at ${globalConfig.toBase58()}`)
  } else {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: payer.publicKey, isSigner: true, isWritable: true }, // authority
        { pubkey: globalConfig, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: false, isWritable: false }, // treasury
        { pubkey: payer.publicKey, isSigner: false, isWritable: false }, // dev_wallet
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: coder.instruction.encode('initialize', {}),
    })
    const sig = await connection.sendTransaction(new Transaction().add(ix), [payer])
    await connection.confirmTransaction(sig, 'confirmed')
    console.log(`✓ initialized global_config at ${globalConfig.toBase58()}  tx=${sig}`)
  }

  // 2. initialize protocol_treasury (required by Buy/Sell/claim paths)
  const [protocolTreasury] = getProtocolTreasuryPda()
  if (await connection.getAccountInfo(protocolTreasury)) {
    console.log(`protocol_treasury already initialized at ${protocolTreasury.toBase58()}`)
  } else {
    const ix = new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: payer.publicKey, isSigner: true, isWritable: true }, // authority
        { pubkey: globalConfig, isSigner: false, isWritable: false },
        { pubkey: protocolTreasury, isSigner: false, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: coder.instruction.encode('initialize_protocol_treasury', {}),
    })
    const sig = await connection.sendTransaction(new Transaction().add(ix), [payer])
    await connection.confirmTransaction(sig, 'confirmed')
    console.log(`✓ initialized protocol_treasury at ${protocolTreasury.toBase58()}  tx=${sig}`)
  }

  console.log(`\nauthority = ${payer.publicKey.toBase58()} (also treasury + dev_wallet)`)
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
