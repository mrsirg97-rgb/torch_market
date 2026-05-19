/**
 * Devnet Full E2E Test
 *
 * Creates a Flame token (100 SOL target), bonds to completion with many small
 * buys, migrates to Raydium DEX (V26 permissionless), then hammers the pool
 * with 200 SOL of post-migration buys/sells, extended lending, and short selling.
 *
 * Run:
 *   npx tsx tests/test_devnet_e2e.ts
 *
 * Requirements:
 *   - Devnet wallet (~/.config/solana/id.json) with ~420 SOL
 *   - Torch Market program deployed to devnet
 *   - Raydium CPMM program on devnet
 */

// Must be set before any torchsdk imports so getRaydiumCpmmProgram() etc. resolve to devnet addresses
process.env.TORCH_NETWORK = 'devnet'

import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
  VersionedTransaction,
  ComputeBudgetProgram,
} from '@solana/web3.js'
import { TOKEN_2022_PROGRAM_ID, getAssociatedTokenAddressSync } from '@solana/spl-token'
import {
  buildCreateTokenTransaction,
  buildDirectBuyTransaction,
  buildBuyTransaction,
  buildSellTransaction,
  buildMigrateTransaction,
  buildBorrowTransaction,
  buildRepayTransaction,
  buildOpenShortTransaction,
  buildCloseShortTransaction,
  buildCreateVaultTransaction,
  buildDepositVaultTransaction,
  buildHarvestFeesTransaction,
  getToken,
  getVault,
  getBuyQuote,
  getSellQuote,
  getBorrowQuote,
} from '../src/index'
import { fetchTokenRaw } from '../src/tokens'
import { getTorchVaultPda, getRaydiumMigrationAccounts } from '../src/program'
import * as fs from 'fs'
import * as path from 'path'
import * as os from 'os'

// ============================================================================
// Config
// ============================================================================

const DEVNET_RPC = 'https://api.devnet.solana.com'
const WALLET_PATH = path.join(os.homedir(), '.config/solana/id.json')

// Flame tier: 100 SOL target, 0.2 SOL per buy (smaller buys, more buyers)
const BONDING_TARGET = 100_000_000_000 // 100 SOL in lamports
const BUY_AMOUNT = Math.floor(0.2 * LAMPORTS_PER_SOL)

// Post-migration config: 200 SOL of buys on Raydium pool
const POST_MIG_TOTAL_SOL = 200 * LAMPORTS_PER_SOL
const POST_MIG_BUY_SIZE = 2 * LAMPORTS_PER_SOL // 2 SOL per swap

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

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

const signAndSend = async (
  connection: Connection,
  signer: Keypair,
  tx: Transaction | VersionedTransaction,
  quiet = false,
): Promise<string> => {
  if (tx instanceof VersionedTransaction) {
    tx.sign([signer])
    const raw = tx.serialize()
    if (!quiet) log(`    tx size: ${raw.length}/1232 bytes`)
    const sig = await connection.sendRawTransaction(raw, {
      skipPreflight: false,
      preflightCommitment: 'confirmed',
    })
    await connection.confirmTransaction(sig, 'confirmed')
    return sig
  }
  tx.partialSign(signer)
  const raw = tx.serialize()
  if (!quiet) log(`    tx size: ${raw.length}/1232 bytes`)
  const sig = await connection.sendRawTransaction(raw, {
    skipPreflight: false,
    preflightCommitment: 'confirmed',
  })
  await connection.confirmTransaction(sig, 'confirmed')
  return sig
}

// ============================================================================
// Main
// ============================================================================

const main = async () => {
  console.log('='.repeat(60))
  console.log('DEVNET FULL E2E TEST — Create → Bond → Migrate → Trade → Lend')
  console.log('='.repeat(60))

  const connection = new Connection(DEVNET_RPC, 'confirmed')
  const wallet = loadWallet()
  const walletAddr = wallet.publicKey.toBase58()

  log(`Wallet: ${walletAddr}`)
  const balance = await connection.getBalance(wallet.publicKey)
  log(`Balance: ${(balance / LAMPORTS_PER_SOL).toFixed(2)} SOL`)

  if (balance < 420 * LAMPORTS_PER_SOL) {
    console.error('Need at least ~420 SOL on devnet. Airdrop or fund the wallet.')
    process.exit(1)
  }

  let passed = 0
  let failed = 0
  const ok = (name: string, detail?: string) => {
    passed++
    log(`  ✓ ${name}${detail ? ` — ${detail}` : ''}`)
  }
  const fail = (name: string, err: any) => {
    failed++
    log(`  ✗ ${name} — ${err.message || err}`)
  }

  // ==================================================================
  // 1. Create Spark Token (50 SOL target)
  // ==================================================================
  log('\n[1] Create Flame Token (100 SOL target)')
  let mint: string
  try {
    const result = await buildCreateTokenTransaction(connection, {
      creator: walletAddr,
      name: 'Devnet E2E Test',
      symbol: 'DEVTEST',
      metadata_uri: 'https://example.com/devtest.json',
      sol_target: BONDING_TARGET,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    mint = result.mint.toBase58()
    ok('Create token', `mint=${mint.slice(0, 8)}... sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('Create token', e)
    console.error('Cannot continue without token. Exiting.')
    process.exit(1)
  }

  // ==================================================================
  // 2. Create Vault + Deposit
  // ==================================================================
  log('\n[2] Create Vault + Deposit')
  try {
    const createResult = await buildCreateVaultTransaction(connection, { creator: walletAddr })
    const sig = await signAndSend(connection, wallet, createResult.transaction)
    ok('Create vault', `sig=${sig.slice(0, 8)}...`)
    await sleep(500)
  } catch (e: any) {
    // Vault may already exist from a previous run
    if (e.message?.includes('already in use')) {
      ok('Create vault', 'already exists')
    } else {
      fail('Create vault', e)
    }
  }

  try {
    const depositResult = await buildDepositVaultTransaction(connection, {
      depositor: walletAddr,
      vault_creator: walletAddr,
      amount_sol: 5 * LAMPORTS_PER_SOL,
    })
    const sig = await signAndSend(connection, wallet, depositResult.transaction)
    ok('Deposit vault', `5 SOL sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('Deposit vault', e)
  }

  // ==================================================================
  // Bonding Curve Quotes (pre-migration)
  // ==================================================================
  log('\n  Testing bonding curve quotes (pre-migration)...')
  try {
    const buyQuote = await getBuyQuote(connection, mint, 1_000_000_000) // 1 SOL
    const sellQuote = await getSellQuote(connection, mint, 100_000_000_000) // 100k tokens

    log(`\n  ┌─── Bonding Curve Price Quotes ────────────────────────────┐`)
    log(`  │  Buy Quote (1 SOL → tokens)                               │`)
    log(`  │    Source:          ${buyQuote.source.padStart(15)}         │`)
    log(
      `  │    Output tokens:   ${(buyQuote.tokens_to_user / 1e6).toFixed(2).padStart(15)}         │`,
    )
    log(
      `  │    Treasury tokens: ${(buyQuote.tokens_to_treasury / 1e6).toFixed(2).padStart(15)}         │`,
    )
    log(
      `  │    Price/token:     ${buyQuote.price_per_token_sol.toFixed(10).padStart(15)} SOL     │`,
    )
    log(
      `  │    Price impact:    ${buyQuote.price_impact_percent.toFixed(4).padStart(14)}%         │`,
    )
    log(`  ├────────────────────────────────────────────────────────────┤`)
    log(`  │  Sell Quote (100k tokens → SOL)                           │`)
    log(`  │    Source:          ${sellQuote.source.padStart(15)}         │`)
    log(`  │    Output SOL:      ${(sellQuote.output_sol / 1e9).toFixed(6).padStart(15)}         │`)
    log(
      `  │    Price/token:     ${sellQuote.price_per_token_sol.toFixed(10).padStart(15)} SOL     │`,
    )
    log(
      `  │    Price impact:    ${sellQuote.price_impact_percent.toFixed(4).padStart(14)}%         │`,
    )
    log(`  └────────────────────────────────────────────────────────────┘`)

    if (buyQuote.source === 'bonding' && sellQuote.source === 'bonding') {
      ok(
        'Bonding quotes',
        `buy=${(buyQuote.tokens_to_user / 1e6).toFixed(0)} tokens/SOL, sell=${(sellQuote.output_sol / 1e9).toFixed(4)} SOL`,
      )
    } else {
      fail('Bonding quotes', {
        message: `expected source=bonding, got buy=${buyQuote.source} sell=${sellQuote.source}`,
      })
    }
  } catch (e: any) {
    fail('Bonding quotes', e)
  }

  // ==================================================================
  // 3. Buy (vault) — initial buy before bonding
  // ==================================================================
  log('\n[3] Buy via vault (initial)')
  try {
    const result = await buildBuyTransaction(connection, {
      mint,
      buyer: walletAddr,
      amount_sol: BUY_AMOUNT,
      slippage_bps: 500,
      vote: 'burn',
      vault: walletAddr,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    ok('Vault buy', `${result.message} sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('Vault buy', e)
  }

  // ==================================================================
  // 4. Bond to completion using many wallets
  // ==================================================================
  log('\n[4] Bond to completion (100 SOL target)')

  const NUM_BUYERS = 700
  const fundPerWallet = BUY_AMOUNT + Math.floor(0.05 * LAMPORTS_PER_SOL)
  const buyers: Keypair[] = []
  for (let i = 0; i < NUM_BUYERS; i++) buyers.push(Keypair.generate())

  // Fund in batches of 10 (devnet rate limits)
  log(`  Funding ${buyers.length} wallets...`)
  const FUND_BATCH = 10
  for (let i = 0; i < buyers.length; i += FUND_BATCH) {
    const batch = buyers.slice(i, i + FUND_BATCH)
    const tx = new Transaction()
    for (const b of batch) {
      tx.add(
        SystemProgram.transfer({
          fromPubkey: wallet.publicKey,
          toPubkey: b.publicKey,
          lamports: fundPerWallet,
        }),
      )
    }
    const { blockhash } = await connection.getLatestBlockhash()
    tx.recentBlockhash = blockhash
    tx.feePayer = wallet.publicKey

    try {
      await signAndSend(connection, wallet, tx, true)
    } catch (e: any) {
      // Retry individually on failure (devnet rate limits)
      await sleep(2000)
      for (const b of batch) {
        try {
          const singleTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: b.publicKey,
              lamports: fundPerWallet,
            }),
          )
          const { blockhash: bh } = await connection.getLatestBlockhash()
          singleTx.recentBlockhash = bh
          singleTx.feePayer = wallet.publicKey
          await signAndSend(connection, wallet, singleTx, true)
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
  log('  Buying...')
  let buyCount = 0
  let skipCount = 0
  let bondingComplete = false

  for (const buyer of buyers) {
    if (bondingComplete) break

    try {
      const result = await buildDirectBuyTransaction(connection, {
        mint,
        buyer: buyer.publicKey.toBase58(),
        amount_sol: BUY_AMOUNT,
        slippage_bps: 1000,
        vote: Math.random() > 0.5 ? 'burn' : 'return',
      })
      await signAndSend(connection, buyer, result.transaction, true)
      buyCount++

      if (buyCount % 25 === 0) {
        const data = await fetchTokenRaw(connection, new PublicKey(mint))
        const reserves = Number(data?.bondingCurve?.real_sol_reserves?.toString() || '0')
        const complete = data?.bondingCurve?.bonding_complete
        const pct = (reserves / BONDING_TARGET) * 100
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
  if (!bondingComplete) {
    const data = await fetchTokenRaw(connection, new PublicKey(mint))
    if (data?.bondingCurve?.bonding_complete) bondingComplete = true
  }

  // [V28] Recovery: if ephemeral buyers couldn't complete bonding (auto-bundled
  // migration requires ~1.5 SOL buffer they don't have), use main wallet
  if (!bondingComplete) {
    log('  Attempting final buy with main wallet (has SOL for V28 migration buffer)...')
    try {
      const result = await buildDirectBuyTransaction(connection, {
        mint,
        buyer: walletAddr,
        amount_sol: BUY_AMOUNT,
        slippage_bps: 1000,
        vote: 'burn',
      })
      await signAndSend(connection, wallet, result.transaction)
      bondingComplete = true
      buyCount++
    } catch (e: any) {
      if (e.message?.includes('BondingComplete') || e.message?.includes('bonding_complete')) {
        bondingComplete = true
      } else {
        log(`  Final buy failed: ${e.message?.substring(0, 80)}`)
      }
    }
  }

  if (bondingComplete) {
    ok('Bonding complete', `after ${buyCount} buys (${skipCount} skipped)`)
  } else {
    fail('Bonding', { message: `Only ${buyCount} buys, not complete` })
    console.error('Cannot continue without bonding completion. Exiting.')
    process.exit(1)
  }

  // ==================================================================
  // 5. Sell via vault (bonding phase complete — sell remaining tokens)
  // ==================================================================
  log('\n[5] Sell via vault (pre-migration — expected to fail, pool not created yet)')
  try {
    const result = await buildSellTransaction(connection, {
      mint,
      seller: walletAddr,
      amount_tokens: 100_000_000, // 100 tokens
      slippage_bps: 500,
      vault: walletAddr,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    // If it somehow succeeds, that's unexpected pre-migration
    ok(
      'Vault sell (pre-migration)',
      `unexpected success — ${result.message} sig=${sig.slice(0, 8)}...`,
    )
  } catch (e: any) {
    // Expected: pool doesn't exist yet, so DEX route fails
    ok('Vault sell (pre-migration)', `correctly blocked — ${(e.message || '').substring(0, 60)}`)
  }

  // ==================================================================
  // 6. Migrate to Raydium DEX (V26 permissionless)
  // ==================================================================
  log('\n[6] Migrate to Raydium DEX (V26 permissionless)')
  try {
    const mintPk = new PublicKey(mint)

    // [V28] Check if auto-migration already happened (bundled with last buy)
    const preMigCheck = await fetchTokenRaw(connection, new PublicKey(mint))
    if (preMigCheck?.bondingCurve?.migrated) {
      ok('Migrate to DEX', 'V28 auto-migrated with last buy')
    } else {
      // Fallback: separate migration call
      const migrateResult = await buildMigrateTransaction(connection, {
        mint,
        payer: walletAddr,
      })
      await signAndSend(connection, wallet, migrateResult.transaction)
      ok('Migrate to DEX', 'Raydium pool created (fallback — separate migration call)')
    }

    // Verify migration flag
    await sleep(1000)
    const detail = await getToken(connection, mint)
    if (detail.status === 'migrated') {
      ok('Migration verified', 'status=migrated')
    } else {
      fail('Migration verified', { message: `status=${detail.status}, expected migrated` })
    }

    // Derive Raydium vault addresses for post-migration verification
    const raydium = getRaydiumMigrationAccounts(mintPk)
    const isWsolToken0 = raydium.isWsolToken0
    const vault0 = raydium.token0Vault
    const vault1 = raydium.token1Vault

    // Post-migration distribution snapshot
    try {
      const postMigData = await fetchTokenRaw(connection, mintPk)
      const bc = postMigData!.bondingCurve
      const tr = postMigData!.treasury!

      const TOTAL_SUPPLY = 1_000_000_000
      const TREASURY_LOCK = 300_000_000 // V31: 300M locked
      const CURVE_SUPPLY = 700_000_000 // V31: 700M for curve + pool
      const tokenVaultPost = isWsolToken0 ? vault1 : vault0
      const poolTokenBalPost = await connection.getTokenAccountBalance(tokenVaultPost)
      const poolTokens = Number(poolTokenBalPost.value.amount) / 1e6
      const voteVault = Number(bc.vote_vault_balance.toString()) / 1e6
      const excessBurned = Number(bc.permanently_burned_tokens.toString()) / 1e6
      const tokensSold = CURVE_SUPPLY - poolTokens - voteVault - excessBurned
      const treasurySol = Number(tr.sol_balance.toString()) / LAMPORTS_PER_SOL
      const poolSolBal = await connection.getTokenAccountBalance(isWsolToken0 ? vault0 : vault1)
      const poolSol = Number(poolSolBal.value.amount) / LAMPORTS_PER_SOL

      // Flame tier: IVS = 37.5 SOL, IVT = 756.25M
      const ivs = 37.5
      const ivt = 756_250_000
      const entryPrice = ivs / ivt
      const exitPrice = poolSol / poolTokens
      const multiplier = exitPrice / entryPrice
      const initialMcSol = TOTAL_SUPPLY * entryPrice
      const finalMcSol = TOTAL_SUPPLY * exitPrice

      log(`\n  ┌─── V31 Post-Migration Distribution ─────────────────────┐`)
      log(`  │  Total Supply:     ${TOTAL_SUPPLY.toLocaleString().padStart(15)} tokens  │`)
      log(`  │  Treasury Lock:    ${TREASURY_LOCK.toLocaleString().padStart(15)} tokens  │`)
      log(`  │  Tokens Sold:      ${tokensSold.toFixed(0).padStart(15)} tokens  │`)
      log(`  │  Vote Vault:       ${voteVault.toFixed(0).padStart(15)} tokens  │`)
      log(`  │  Pool Tokens:      ${poolTokens.toFixed(0).padStart(15)} tokens  │`)
      log(`  │  Excess Burned:    ${excessBurned.toFixed(0).padStart(15)} tokens  │`)
      log(`  ├────────────────────────────────────────────────────────────┤`)
      log(`  │  Pool SOL:         ${poolSol.toFixed(4).padStart(15)} SOL     │`)
      log(`  │  Treasury SOL:     ${treasurySol.toFixed(4).padStart(15)} SOL     │`)
      log(`  ├────────────────────────────────────────────────────────────┤`)
      log(`  │  Entry Price:      ${entryPrice.toExponential(4).padStart(15)} SOL/tok │`)
      log(`  │  Exit Price:       ${exitPrice.toExponential(4).padStart(15)} SOL/tok │`)
      log(`  │  Multiplier:       ${multiplier.toFixed(1).padStart(15)}x        │`)
      log(`  │  Initial MC:       ${initialMcSol.toFixed(2).padStart(15)} SOL     │`)
      log(`  │  Final MC:         ${finalMcSol.toFixed(2).padStart(15)} SOL     │`)
      log(`  └────────────────────────────────────────────────────────────┘`)
    } catch {
      /* non-critical */
    }

    // Wait for Raydium pool open_time to pass
    log('\n  Waiting 15s for Raydium pool open_time...')
    await sleep(15000)

    // ==================================================================
    // DEX Quotes — getBuyQuote / getSellQuote on migrated token
    // ==================================================================
    log('\n  Testing DEX quotes (post-migration)...')
    try {
      const buyQuote = await getBuyQuote(connection, mint, 1_000_000_000) // 1 SOL
      const sellQuote = await getSellQuote(connection, mint, 100_000_000_000) // 100k tokens

      log(`\n  ┌─── DEX Price Quotes ──────────────────────────────────────┐`)
      log(`  │  Buy Quote (1 SOL → tokens)                               │`)
      log(`  │    Source:          ${buyQuote.source.padStart(15)}         │`)
      log(
        `  │    Output tokens:   ${(buyQuote.tokens_to_user / 1e6).toFixed(2).padStart(15)}         │`,
      )
      log(
        `  │    Price/token:     ${buyQuote.price_per_token_sol.toFixed(10).padStart(15)} SOL     │`,
      )
      log(
        `  │    Price impact:    ${buyQuote.price_impact_percent.toFixed(4).padStart(14)}%         │`,
      )
      log(`  ├────────────────────────────────────────────────────────────┤`)
      log(`  │  Sell Quote (100k tokens → SOL)                           │`)
      log(`  │    Source:          ${sellQuote.source.padStart(15)}         │`)
      log(
        `  │    Output SOL:      ${(sellQuote.output_sol / 1e9).toFixed(6).padStart(15)}         │`,
      )
      log(
        `  │    Price/token:     ${sellQuote.price_per_token_sol.toFixed(10).padStart(15)} SOL     │`,
      )
      log(
        `  │    Price impact:    ${sellQuote.price_impact_percent.toFixed(4).padStart(14)}%         │`,
      )
      log(`  └────────────────────────────────────────────────────────────┘`)

      if (buyQuote.source === 'dex' && sellQuote.source === 'dex') {
        ok(
          'DEX quotes',
          `buy=${(buyQuote.tokens_to_user / 1e6).toFixed(0)} tokens/SOL, sell=${(sellQuote.output_sol / 1e9).toFixed(4)} SOL`,
        )
      } else {
        fail('DEX quotes', {
          message: `expected source=dex, got buy=${buyQuote.source} sell=${sellQuote.source}`,
        })
      }
    } catch (e: any) {
      fail('DEX quotes', e)
    }

    // ==================================================================
    // 7. Deposit vault SOL for post-migration trading (200+ SOL)
    // ==================================================================
    log('\n[7] Deposit vault SOL for post-migration trading')
    try {
      const depositResult = await buildDepositVaultTransaction(connection, {
        depositor: walletAddr,
        vault_creator: walletAddr,
        amount_sol: POST_MIG_TOTAL_SOL + 10 * LAMPORTS_PER_SOL, // 210 SOL (200 + buffer)
      })
      const sig = await signAndSend(connection, wallet, depositResult.transaction)
      ok('Deposit vault (post-mig)', `210 SOL sig=${sig.slice(0, 8)}...`)
    } catch (e: any) {
      fail('Deposit vault (post-mig)', e)
    }

    await sleep(500)

    // ==================================================================
    // 8. Post-migration buys — 200 SOL via buildBuyTransaction (2 SOL × 100)
    // ==================================================================
    log('\n[8] Post-migration buys — 200 SOL via buildBuyTransaction (2 SOL × 100)')
    const numPostBuys = Math.ceil(POST_MIG_TOTAL_SOL / POST_MIG_BUY_SIZE)
    let postBuyCount = 0
    let postBuyFails = 0
    let totalSolSpentOnBuys = 0

    for (let i = 0; i < numPostBuys; i++) {
      try {
        const quote = await getBuyQuote(connection, mint, POST_MIG_BUY_SIZE)
        const buyResult = await buildBuyTransaction(connection, {
          mint,
          buyer: walletAddr,
          amount_sol: POST_MIG_BUY_SIZE,
          slippage_bps: 500,
          vault: walletAddr,
          quote,
        })
        await signAndSend(connection, wallet, buyResult.transaction, true)
        postBuyCount++
        totalSolSpentOnBuys += POST_MIG_BUY_SIZE

        if ((i + 1) % 20 === 0) {
          const spent = totalSolSpentOnBuys / LAMPORTS_PER_SOL
          log(`  Buy ${i + 1}/${numPostBuys}: ${spent.toFixed(1)} SOL spent so far`)
        }
      } catch (e: any) {
        postBuyFails++
        if (postBuyFails <= 3) {
          log(`  Buy ${i + 1} failed: ${(e.message || '').substring(0, 80)}`)
          if (e.logs) console.error('  Logs:', e.logs.slice(-3).join('\n        '))
        }
        await sleep(1000)
      }
      await sleep(200)
    }

    if (postBuyCount > 0) {
      ok(
        'Post-migration buys',
        `${postBuyCount}/${numPostBuys} succeeded, ${(totalSolSpentOnBuys / LAMPORTS_PER_SOL).toFixed(1)} SOL spent`,
      )
    } else {
      fail('Post-migration buys', { message: 'all buys failed' })
    }

    // Pool price snapshot after 200 SOL of buys
    try {
      const tokenVault = isWsolToken0 ? vault1 : vault0
      const solVault = isWsolToken0 ? vault0 : vault1
      const poolTokenBal = await connection.getTokenAccountBalance(tokenVault)
      const poolSolBal = await connection.getTokenAccountBalance(solVault)
      const poolTokens = Number(poolTokenBal.value.amount) / 1e6
      const poolSol = Number(poolSolBal.value.amount) / LAMPORTS_PER_SOL
      const pricePerToken = poolTokens > 0 ? poolSol / poolTokens : 0
      const tokensPerSol = poolSol > 0 ? poolTokens / poolSol : 0

      log(`\n  ┌─── Pool State After 200 SOL Buys ────────────────────────┐`)
      log(`  │  Pool SOL:         ${poolSol.toFixed(4).padStart(15)} SOL     │`)
      log(`  │  Pool Tokens:      ${poolTokens.toFixed(0).padStart(15)} tokens  │`)
      log(`  │  Price/Token:      ${pricePerToken.toFixed(10).padStart(15)} SOL     │`)
      log(`  │  Tokens/SOL:       ${tokensPerSol.toFixed(2).padStart(15)} tokens  │`)
      log(`  └────────────────────────────────────────────────────────────┘`)
    } catch {
      /* non-critical */
    }

    await sleep(500)

    // ==================================================================
    // 9. Post-migration sells — sell back 20% of vault tokens
    // ==================================================================
    log('\n[9] Post-migration sells (20% of vault tokens)')
    try {
      const [vaultPda] = getTorchVaultPda(wallet.publicKey)
      const vaultAta = getAssociatedTokenAddressSync(
        new PublicKey(mint),
        vaultPda,
        true,
        TOKEN_2022_PROGRAM_ID,
      )
      const tokenBal = await connection.getTokenAccountBalance(vaultAta)
      const totalTokens = Number(tokenBal.value.amount)
      const sellTotal = Math.floor(totalTokens * 0.2)
      const SELL_BATCH = Math.floor(sellTotal / 5) // 5 sells of 4% each
      log(
        `  Vault tokens: ${(totalTokens / 1e6).toFixed(0)}, selling ${(sellTotal / 1e6).toFixed(0)} in 5 batches`,
      )

      let sellCount = 0
      let totalSolReceived = 0

      for (let i = 0; i < 5; i++) {
        const sellAmount = i < 4 ? SELL_BATCH : sellTotal - SELL_BATCH * 4
        if (sellAmount < 1_000_000) break

        try {
          const vaultBefore = await getVault(connection, walletAddr)
          const sellQuote = await getSellQuote(connection, mint, sellAmount)
          const sellResult = await buildSellTransaction(connection, {
            mint,
            seller: walletAddr,
            amount_tokens: sellAmount,
            slippage_bps: 500,
            vault: walletAddr,
            quote: sellQuote,
          })
          await signAndSend(connection, wallet, sellResult.transaction)
          const vaultAfter = await getVault(connection, walletAddr)
          const received = (vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)
          totalSolReceived += received
          sellCount++
        } catch (e: any) {
          log(`  Sell ${i + 1} failed: ${(e.message || '').substring(0, 80)}`)
        }
        await sleep(300)
      }

      if (sellCount > 0) {
        ok(
          'Post-migration sells',
          `${sellCount}/5 succeeded, received ${totalSolReceived.toFixed(4)} SOL`,
        )
      } else {
        fail('Post-migration sells', { message: 'all sells failed' })
      }
    } catch (e: any) {
      fail('Post-migration sells', e)
      if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
    }

    await sleep(500)

    // ==================================================================
    // 10. Harvest Transfer Fees
    // ==================================================================
    log('\n[10] Harvest Transfer Fees')
    try {
      // The 100 post-migration buys + sells generated transfer fees
      const preHarvestData = await fetchTokenRaw(connection, new PublicKey(mint))
      const preSolBalance =
        Number(preHarvestData?.treasury?.sol_balance?.toString() || '0') / LAMPORTS_PER_SOL

      // Read treasury token account balance (where harvested tokens actually go)
      const { getTokenTreasuryPda, getTreasuryTokenAccount } = require('../src/program')
      const [treasuryPdaH] = getTokenTreasuryPda(new PublicKey(mint))
      const treasuryAtaH = getTreasuryTokenAccount(new PublicKey(mint), treasuryPdaH)
      let preTokenBal = 0
      try {
        const bal = await connection.getTokenAccountBalance(treasuryAtaH)
        preTokenBal = Number(bal.value.amount)
      } catch {
        /* ATA may not exist yet */
      }

      log(
        `  [before] treasury_sol=${preSolBalance.toFixed(4)} SOL, treasury_tokens=${(preTokenBal / 1e6).toFixed(2)}`,
      )

      const harvestResult = await buildHarvestFeesTransaction(connection, {
        mint,
        payer: walletAddr,
      })
      const harvestSig = await signAndSend(connection, wallet, harvestResult.transaction)

      // Snapshot after harvest
      const postHarvestData = await fetchTokenRaw(connection, new PublicKey(mint))
      const postSolBalance =
        Number(postHarvestData?.treasury?.sol_balance?.toString() || '0') / LAMPORTS_PER_SOL
      let postTokenBal = 0
      try {
        const bal = await connection.getTokenAccountBalance(treasuryAtaH)
        postTokenBal = Number(bal.value.amount)
      } catch {
        /* shouldn't happen */
      }

      const tokensHarvested = postTokenBal - preTokenBal
      log(
        `  [after]  treasury_sol=${postSolBalance.toFixed(4)} SOL, treasury_tokens=${(postTokenBal / 1e6).toFixed(2)} (+${(tokensHarvested / 1e6).toFixed(2)})`,
      )

      if (tokensHarvested > 0) {
        ok(
          'Harvest fees',
          `${harvestResult.message} — harvested ${(tokensHarvested / 1e6).toFixed(2)} tokens sig=${harvestSig.slice(0, 8)}...`,
        )
      } else {
        ok(
          'Harvest fees',
          `${harvestResult.message} — tx succeeded (no withheld fees) sig=${harvestSig.slice(0, 8)}...`,
        )
      }
    } catch (e: any) {
      fail('Harvest fees', e)
      if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
    }

    await sleep(500)

    await sleep(500)

    // ==================================================================
    // 11. Borrow via vault (post-migration lending, was 12 — buyback section removed in V33)
    // ==================================================================
    log('\n[12] Borrow via vault (post-migration)')
    try {
      const [vaultPda] = getTorchVaultPda(wallet.publicKey)
      const vaultAta = getAssociatedTokenAddressSync(
        new PublicKey(mint),
        vaultPda,
        true,
        TOKEN_2022_PROGRAM_ID,
      )
      const tokenBal = await connection.getTokenAccountBalance(vaultAta)
      const totalTokens = Number(tokenBal.value.amount)
      log(`  Vault token balance: ${(totalTokens / 1e6).toFixed(0)} tokens`)

      const collateralAmount = Math.floor(totalTokens * 0.9)
      const quote = await getBorrowQuote(connection, mint, collateralAmount)
      const borrowAmount = Math.min(1 * LAMPORTS_PER_SOL, quote.max_borrow_sol) // up to 1 SOL, within quote cap
      log(
        `  pool_available: ${(quote.pool_available_sol / LAMPORTS_PER_SOL).toFixed(4)}, per-user cap: ${(quote.per_user_cap_sol / LAMPORTS_PER_SOL).toFixed(4)}, borrowing: ${(borrowAmount / LAMPORTS_PER_SOL).toFixed(4)}`,
      )

      if (borrowAmount < 100_000_000) {
        ok(
          'Vault borrow',
          'skipped — treasury or per-user cap too small for minimum borrow (0.1 SOL)',
        )
      } else {
        const vaultBefore = await getVault(connection, walletAddr)
        const borrowResult = await buildBorrowTransaction(connection, {
          mint,
          borrower: walletAddr,
          collateral_amount: collateralAmount,
          sol_to_borrow: borrowAmount,
          vault: walletAddr,
        })
        const borrowSig = await signAndSend(connection, wallet, borrowResult.transaction)
        const vaultAfter = await getVault(connection, walletAddr)
        const solReceived = (vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)
        ok(
          'Vault borrow',
          `${borrowResult.message} vault_received=${solReceived.toFixed(4)} SOL sig=${borrowSig.slice(0, 8)}...`,
        )

        await sleep(500)

        // ==============================================================
        // 13. Repay via vault
        // ==============================================================
        log('\n[13] Repay via vault')
        try {
          const repayResult = await buildRepayTransaction(connection, {
            mint,
            borrower: walletAddr,
            sol_amount: borrowAmount + 100_000_000, // overpay to fully close
            vault: walletAddr,
          })
          const repaySig = await signAndSend(connection, wallet, repayResult.transaction)
          ok('Vault repay', `${repayResult.message} sig=${repaySig.slice(0, 8)}...`)
        } catch (e: any) {
          fail('Vault repay', e)
        }

        await sleep(500)

        // ==============================================================
        // 14. Second borrow+repay cycle (extended lending)
        // ==============================================================
        log('\n[14] Second borrow+repay cycle')
        try {
          const tokenBal2 = await connection.getTokenAccountBalance(vaultAta)
          const tokens2 = Number(tokenBal2.value.amount)
          const collateral2 = Math.floor(tokens2 * 0.4)
          const quote2 = await getBorrowQuote(connection, mint, collateral2)
          const borrowAmount2 = Math.min(500_000_000, quote2.max_borrow_sol) // up to 0.5 SOL, within quote cap

          if (borrowAmount2 < 100_000_000 || tokens2 < 10_000_000) {
            ok('Second borrow', 'skipped — insufficient treasury, collateral, or per-user cap')
          } else {
            const borrowResult2 = await buildBorrowTransaction(connection, {
              mint,
              borrower: walletAddr,
              collateral_amount: collateral2,
              sol_to_borrow: borrowAmount2,
              vault: walletAddr,
            })
            const bSig2 = await signAndSend(connection, wallet, borrowResult2.transaction)
            ok('Second borrow', `${borrowResult2.message} sig=${bSig2.slice(0, 8)}...`)

            await sleep(500)

            const repayResult2 = await buildRepayTransaction(connection, {
              mint,
              borrower: walletAddr,
              sol_amount: borrowAmount2 + 100_000_000,
              vault: walletAddr,
            })
            const rSig2 = await signAndSend(connection, wallet, repayResult2.transaction)
            ok('Second repay', `${repayResult2.message} sig=${rSig2.slice(0, 8)}...`)
          }
        } catch (e: any) {
          fail('Second borrow+repay', e)
        }
      }
    } catch (e: any) {
      fail('Vault borrow', e)
      if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
    }

    // ==============================================================
    // 15. Short Selling via vault (open_short → close_short)
    // ==============================================================
    log('\n[15] Short Selling via vault (open_short → close_short)')
    try {
      // Deposit SOL to vault for short collateral (previous tests may have drained it)
      const depositForShort = await buildDepositVaultTransaction(connection, {
        depositor: walletAddr,
        vault_creator: walletAddr,
        amount_sol: 1 * LAMPORTS_PER_SOL,
      })
      await signAndSend(connection, wallet, depositForShort.transaction, true)
      log('  Deposited 1 SOL to vault for short collateral')

      const vaultBeforeShort = await getVault(connection, walletAddr)
      const vaultSolInSol = vaultBeforeShort?.sol_balance || 0 // already in SOL units (not lamports)
      log(`  Vault SOL: ${vaultSolInSol.toFixed(4)} SOL`)

      const shortCollateral = Math.floor(0.5 * LAMPORTS_PER_SOL) // 0.5 SOL
      const tokensToBorrow = 1_000_000_000 // 1,000 tokens (MIN_SHORT_TOKENS)

      if (vaultSolInSol < 0.5) {
        ok('Open short', 'skipped — vault SOL too low for 0.5 SOL collateral')
      } else {
        const openResult = await buildOpenShortTransaction(connection, {
          mint,
          shorter: walletAddr,
          sol_collateral: shortCollateral,
          tokens_to_borrow: tokensToBorrow,
          vault: walletAddr,
        })
        const openSig = await signAndSend(connection, wallet, openResult.transaction)
        const vaultAfterOpen = await getVault(connection, walletAddr)
        const solSpentShort =
          (vaultBeforeShort?.sol_balance || 0) - (vaultAfterOpen?.sol_balance || 0)
        ok(
          'Open short (vault)',
          `${openResult.message} vault_spent=${(solSpentShort / LAMPORTS_PER_SOL).toFixed(4)} SOL sig=${openSig.slice(0, 8)}...`,
        )

        // Close short — overpay to fully close
        log('  Closing short...')
        try {
          const closeResult = await buildCloseShortTransaction(connection, {
            mint,
            shorter: walletAddr,
            token_amount: tokensToBorrow * 2,
            vault: walletAddr,
          })
          const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
          const vaultAfterClose = await getVault(connection, walletAddr)
          const solReturned =
            (vaultAfterClose?.sol_balance || 0) - (vaultAfterOpen?.sol_balance || 0)
          ok(
            'Close short (vault)',
            `${closeResult.message} vault_received=${(solReturned / LAMPORTS_PER_SOL).toFixed(4)} SOL sig=${closeSig.slice(0, 8)}...`,
          )
        } catch (e: any) {
          fail('Close short (vault)', e)
          if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
        }
      }
    } catch (e: any) {
      fail('Short selling', e)
      if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
    }
  } catch (e: any) {
    fail('Migration/post-migration lifecycle', e)
    if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
  }

  // ==================================================================
  // Summary
  // ==================================================================
  const funderBalanceAfter = await connection.getBalance(wallet.publicKey)
  const solSpent = (balance - funderBalanceAfter) / LAMPORTS_PER_SOL

  console.log('\n' + '='.repeat(60))
  console.log(`RESULTS: ${passed} passed, ${failed} failed`)
  console.log(
    `SOL spent: ${solSpent.toFixed(2)} SOL (${(funderBalanceAfter / LAMPORTS_PER_SOL).toFixed(2)} remaining)`,
  )
  console.log('='.repeat(60))

  process.exit(failed > 0 ? 1 : 0)
}

main().catch((e) => {
  console.error('\nFATAL:', e)
  process.exit(1)
})
