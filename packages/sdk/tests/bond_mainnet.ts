/**
 * Mainnet Auto-Bond Script
 *
 * Distributes SOL from the mainnet deploy wallet across many ephemeral wallets
 * and buys a specified token until bonding completes.
 *
 * Usage:
 *   npx tsx tests/bond_mainnet.ts <MINT_ADDRESS>            # bond
 *   npx tsx tests/bond_mainnet.ts <MINT_ADDRESS> --collect   # post-migration: collect Token-2022 tokens
 */

import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
} from '@solana/web3.js'
import {
  createAssociatedTokenAccountIdempotentInstruction,
  createTransferCheckedInstruction,
  getAssociatedTokenAddressSync,
  TOKEN_2022_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAccount,
} from '@solana/spl-token'
import { buildDirectBuyTransaction } from '../src/index'
import { fetchTokenRaw } from '../src/tokens'
import * as fs from 'fs'
import * as path from 'path'
import * as os from 'os'

// ============================================================================
// Config
// ============================================================================

const MAINNET_RPC = 'https://torch-market-rpc.mrsirg97.workers.dev'
const WALLET_PATH = path.join(
  os.homedir(),
  'Projects/burnfun/torch_market/keys/mainnet-deploy-wallet.json',
)

const BUY_LAMPORTS = Math.floor(0.2 * LAMPORTS_PER_SOL)
const MAX_SOL_SPEND = 59 * LAMPORTS_PER_SOL
const FUND_BATCH = 20
const TOKEN_DECIMALS = 6

// ============================================================================
// Helpers
// ============================================================================

const loadWallet = (): Keypair => {
  const raw = JSON.parse(fs.readFileSync(WALLET_PATH, 'utf-8'))
  return Keypair.fromSecretKey(Uint8Array.from(raw))
}

const log = (msg: string) => {
  const ts = new Date().toISOString().substr(11, 8)
  console.log(`[${ts}] ${msg}`)
}

const signAndSend = async (
  connection: Connection,
  signer: Keypair,
  tx: Transaction,
): Promise<string> => {
  tx.partialSign(signer)
  const sig = await connection.sendRawTransaction(tx.serialize(), {
    skipPreflight: false,
    preflightCommitment: 'confirmed',
  })
  await connection.confirmTransaction(sig, 'confirmed')
  return sig
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

// ============================================================================
// Collect mode — transfer Token-2022 tokens back to authority
// ============================================================================

const collectTokens = async (mintArg: string) => {
  const mintPk = new PublicKey(mintArg)
  const connection = new Connection(MAINNET_RPC, 'confirmed')
  const authority = loadWallet()

  // Find saved keypairs file
  const prefix = mintArg.substring(0, 8)
  const keypairFile = path.join(__dirname, `bond_wallets_${prefix}.json`)
  if (!fs.existsSync(keypairFile)) {
    console.error(`Keypair file not found: ${keypairFile}`)
    console.error('The bond wallets must have been saved during the bond phase.')
    process.exit(1)
  }

  const savedKeys: number[][] = JSON.parse(fs.readFileSync(keypairFile, 'utf-8'))
  const wallets = savedKeys.map((k) => Keypair.fromSecretKey(Uint8Array.from(k)))

  console.log('='.repeat(60))
  console.log('COLLECT TOKEN-2022 TOKENS')
  console.log('='.repeat(60))
  log(`Authority: ${authority.publicKey.toBase58()}`)
  log(`Mint:      ${mintArg}`)
  log(`Wallets:   ${wallets.length}`)

  // Authority ATA for this mint (Token-2022)
  const authorityAta = getAssociatedTokenAddressSync(
    mintPk,
    authority.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // Ensure authority ATA exists
  const createAtaTx = new Transaction().add(
    createAssociatedTokenAccountIdempotentInstruction(
      authority.publicKey,
      authorityAta,
      authority.publicKey,
      mintPk,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )
  const { blockhash: ataBlockhash } = await connection.getLatestBlockhash()
  createAtaTx.recentBlockhash = ataBlockhash
  createAtaTx.feePayer = authority.publicKey
  try {
    await signAndSend(connection, authority, createAtaTx)
    log('Authority ATA created/verified')
  } catch {
    log('Authority ATA already exists')
  }

  // Batch 3 transfers per tx
  const COLLECT_BATCH = 3
  let collected = 0
  let skipped = 0

  for (let i = 0; i < wallets.length; i += COLLECT_BATCH) {
    const batch = wallets.slice(i, i + COLLECT_BATCH)
    const tx = new Transaction()
    const signers: Keypair[] = [authority]
    let hasTransfers = false

    for (const wallet of batch) {
      const walletAta = getAssociatedTokenAddressSync(
        mintPk,
        wallet.publicKey,
        false,
        TOKEN_2022_PROGRAM_ID,
      )

      try {
        const account = await getAccount(connection, walletAta, 'confirmed', TOKEN_2022_PROGRAM_ID)
        const balance = BigInt(account.amount.toString())
        if (balance <= BigInt(0)) {
          skipped++
          continue
        }

        tx.add(
          createTransferCheckedInstruction(
            walletAta,
            mintPk,
            authorityAta,
            wallet.publicKey,
            balance,
            TOKEN_DECIMALS,
            [],
            TOKEN_2022_PROGRAM_ID,
          ),
        )
        signers.push(wallet)
        hasTransfers = true
      } catch {
        skipped++
        continue
      }
    }

    if (!hasTransfers) continue

    const { blockhash } = await connection.getLatestBlockhash()
    tx.recentBlockhash = blockhash
    tx.feePayer = authority.publicKey

    try {
      for (const s of signers) tx.partialSign(s)
      const sig = await connection.sendRawTransaction(tx.serialize(), {
        skipPreflight: false,
        preflightCommitment: 'confirmed',
      })
      await connection.confirmTransaction(sig, 'confirmed')
      collected += signers.length - 1 // minus authority
      log(`  [${collected}] ${sig}`)
    } catch (e: any) {
      log(`  Batch ${Math.floor(i / COLLECT_BATCH)} failed: ${(e.message || '').substring(0, 80)}`)
    }

    await sleep(500)
  }

  console.log('\n' + '='.repeat(60))
  console.log(`Collected from ${collected} wallets, skipped ${skipped}`)
  console.log('='.repeat(60))

  // Check authority balance
  try {
    const authorityAccount = await getAccount(
      connection,
      authorityAta,
      'confirmed',
      TOKEN_2022_PROGRAM_ID,
    )
    log(`Authority token balance: ${Number(authorityAccount.amount) / 10 ** TOKEN_DECIMALS}`)
  } catch {
    log('Could not read authority token balance')
  }
}

// ============================================================================
// Main — Bond mode
// ============================================================================

const main = async () => {
  const mintArg = process.argv[2]
  if (!mintArg) {
    console.error('Usage: npx tsx tests/bond_mainnet.ts <MINT_ADDRESS> [--collect]')
    process.exit(1)
  }

  // --collect mode
  if (process.argv.includes('--collect')) {
    await collectTokens(mintArg)
    return
  }

  const mintPk = new PublicKey(mintArg)
  const connection = new Connection(MAINNET_RPC, 'confirmed')
  const funder = loadWallet()

  console.log('='.repeat(60))
  console.log('MAINNET AUTO-BOND')
  console.log('='.repeat(60))
  log(`Funder: ${funder.publicKey.toBase58()}`)
  log(`Mint:   ${mintArg}`)

  // Check funder balance
  const funderBalance = await connection.getBalance(funder.publicKey)
  log(`Balance: ${(funderBalance / LAMPORTS_PER_SOL).toFixed(2)} SOL`)

  // Read token state
  log('\nReading token state...')
  const tokenData = await fetchTokenRaw(connection, mintPk)
  if (!tokenData) {
    console.error('Token not found!')
    process.exit(1)
  }

  const { bondingCurve: bc } = tokenData
  if (bc.bonding_complete) {
    log('Token bonding already complete!')
    process.exit(0)
  }

  const realSol = Number(bc.real_sol_reserves.toString())
  const targetLamports = Number(bc.bonding_target.toString())
  const solRemaining = (targetLamports - realSol) / LAMPORTS_PER_SOL
  const progressPct = (realSol / targetLamports) * 100

  log(`Target:    ${(targetLamports / LAMPORTS_PER_SOL).toFixed(0)} SOL`)
  log(`Raised:    ${(realSol / LAMPORTS_PER_SOL).toFixed(4)} SOL (${progressPct.toFixed(1)}%)`)
  log(`Remaining: ~${solRemaining.toFixed(2)} SOL`)
  log(`Buy size:  ${(BUY_LAMPORTS / LAMPORTS_PER_SOL).toFixed(2)} SOL per wallet`)

  const fundPerWallet = BUY_LAMPORTS + Math.floor(0.01 * LAMPORTS_PER_SOL) // buy + gas/rent
  const maxByBudget = Math.floor(MAX_SOL_SPEND / fundPerWallet)
  const estBuysNeeded = Math.ceil(((solRemaining * LAMPORTS_PER_SOL) / BUY_LAMPORTS) * 1.15)
  const numWallets = Math.min(estBuysNeeded, maxByBudget)
  const totalSolNeeded = numWallets * fundPerWallet

  log(
    `Wallets:   ${numWallets} (capped at ${(MAX_SOL_SPEND / LAMPORTS_PER_SOL).toFixed(0)} SOL budget)`,
  )
  log(`Total SOL needed: ~${(totalSolNeeded / LAMPORTS_PER_SOL).toFixed(2)} SOL`)

  if (funderBalance < totalSolNeeded + 1 * LAMPORTS_PER_SOL) {
    console.error(
      `Insufficient balance! Have ${(funderBalance / LAMPORTS_PER_SOL).toFixed(2)} SOL, need ~${((totalSolNeeded + LAMPORTS_PER_SOL) / LAMPORTS_PER_SOL).toFixed(2)} SOL`,
    )
    process.exit(1)
  }

  // Generate wallets
  log(`\nGenerating ${numWallets} wallets...`)
  const buyers: Keypair[] = []
  for (let i = 0; i < numWallets; i++) buyers.push(Keypair.generate())

  // Save keypairs immediately (before any funding)
  const prefix = mintArg.substring(0, 8)
  const keypairFile = path.join(__dirname, `bond_wallets_${prefix}.json`)
  const keypairData = buyers.map((b) => Array.from(b.secretKey))
  fs.writeFileSync(keypairFile, JSON.stringify(keypairData))
  log(`Saved ${buyers.length} keypairs to ${keypairFile}`)

  // Fund wallets in batches of 20
  log('Funding wallets...')
  for (let i = 0; i < buyers.length; i += FUND_BATCH) {
    const batch = buyers.slice(i, i + FUND_BATCH)
    const tx = new Transaction()
    for (const b of batch) {
      tx.add(
        SystemProgram.transfer({
          fromPubkey: funder.publicKey,
          toPubkey: b.publicKey,
          lamports: fundPerWallet,
        }),
      )
    }
    const { blockhash } = await connection.getLatestBlockhash()
    tx.recentBlockhash = blockhash
    tx.feePayer = funder.publicKey

    try {
      await signAndSend(connection, funder, tx)
    } catch (e: any) {
      log(
        `  Funding batch ${Math.floor(i / FUND_BATCH)} failed: ${(e.message || '').substring(0, 80)}`,
      )
      // Retry individually
      await sleep(2000)
      for (const b of batch) {
        try {
          const singleTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: funder.publicKey,
              toPubkey: b.publicKey,
              lamports: fundPerWallet,
            }),
          )
          const { blockhash: bh } = await connection.getLatestBlockhash()
          singleTx.recentBlockhash = bh
          singleTx.feePayer = funder.publicKey
          await signAndSend(connection, funder, singleTx)
        } catch {
          /* skip */
        }
        await sleep(500)
      }
    }

    if ((i + FUND_BATCH) % 100 === 0) {
      log(`  Funded ${Math.min(i + FUND_BATCH, buyers.length)}/${buyers.length}`)
    }
    await sleep(200)
  }
  log(`  All ${buyers.length} wallets funded`)

  // Buy until bonding completes
  log('\nBuying...')
  let buyCount = 0
  let skipCount = 0
  let bondingComplete = false

  for (const buyer of buyers) {
    if (bondingComplete) break

    try {
      const result = await buildDirectBuyTransaction(connection, {
        mint: mintArg,
        buyer: buyer.publicKey.toBase58(),
        amount_sol: BUY_LAMPORTS,
        slippage_bps: 1000,
        vote: Math.random() > 0.5 ? 'burn' : 'return',
      })
      await signAndSend(connection, buyer, result.transaction)
      buyCount++

      // Progress check every 25 buys
      if (buyCount % 25 === 0) {
        const data = await fetchTokenRaw(connection, mintPk)
        const reserves = Number(data?.bondingCurve?.real_sol_reserves?.toString() || '0')
        const complete = data?.bondingCurve?.bonding_complete
        const pct = (reserves / targetLamports) * 100
        log(
          `  Buy ${buyCount}: ${(reserves / LAMPORTS_PER_SOL).toFixed(2)} SOL (${pct.toFixed(1)}%)${complete ? ' — COMPLETE!' : ''}`,
        )
        if (complete) {
          bondingComplete = true
          break
        }
      }
    } catch (e: any) {
      const msg = e.message || ''
      if (
        msg.includes('BondingComplete') ||
        msg.includes('bonding_complete') ||
        msg.includes('Bonding curve complete')
      ) {
        bondingComplete = true
        break
      }
      skipCount++
      if (skipCount <= 5) {
        log(`  Buy ${buyCount + 1} skipped: ${msg.substring(0, 80)}`)
      }
      await sleep(1000)
    }
    await sleep(100)
  }

  // Final status
  let finalData = await fetchTokenRaw(connection, mintPk)
  if (!finalData?.bondingCurve?.bonding_complete && !bondingComplete) {
    // Recovery: try funder wallet
    log('\nAttempting final buy with funder wallet (has SOL for migration buffer)...')
    try {
      const result = await buildDirectBuyTransaction(connection, {
        mint: mintArg,
        buyer: funder.publicKey.toBase58(),
        amount_sol: BUY_LAMPORTS,
        slippage_bps: 1000,
        vote: 'burn',
      })
      await signAndSend(connection, funder, result.transaction)
      bondingComplete = true
      buyCount++
      log('  Final buy succeeded — bonding complete')
    } catch (e: any) {
      if (e.message?.includes('BondingComplete') || e.message?.includes('bonding_complete')) {
        bondingComplete = true
      } else {
        log(`  Final buy failed: ${(e.message || '').substring(0, 80)}`)
      }
    }
    finalData = await fetchTokenRaw(connection, mintPk)
  } else {
    bondingComplete = true
  }

  const finalReserves = Number(finalData?.bondingCurve?.real_sol_reserves?.toString() || '0')
  const finalPct = (finalReserves / targetLamports) * 100

  console.log('\n' + '='.repeat(60))
  if (bondingComplete) {
    console.log('BONDING COMPLETE!')
  } else {
    console.log('BONDING NOT COMPLETE — may need more SOL or wallets')
  }
  console.log('='.repeat(60))
  console.log(`  Buys:     ${buyCount}`)
  console.log(`  Skipped:  ${skipCount}`)
  console.log(
    `  Reserves: ${(finalReserves / LAMPORTS_PER_SOL).toFixed(4)} SOL (${finalPct.toFixed(1)}%)`,
  )
  console.log(`  Target:   ${(targetLamports / LAMPORTS_PER_SOL).toFixed(0)} SOL`)

  const funderBalanceAfter = await connection.getBalance(funder.publicKey)
  console.log(`  Funder:   ${(funderBalanceAfter / LAMPORTS_PER_SOL).toFixed(2)} SOL remaining`)
  console.log(`  Keypairs: ${keypairFile}`)
  console.log('='.repeat(60))

  process.exit(bondingComplete ? 0 : 1)
}

main().catch((e) => {
  console.error('\nFATAL:', e)
  process.exit(1)
})
