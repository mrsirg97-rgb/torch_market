/**
 * SDK E2E Test against Surfpool (mainnet fork)
 *
 * Tests: create token → vault lifecycle → buy (direct + vault) → sell → messages
 * Then: bond to completion → migrate → [V21] leverage stress tests (long + short)
 * Leverage: getLendingInfo → open long → getPosition(long) → partial close → full close
 *         → open short → getPosition(short) → partial close → full close
 *         → vault swap (buy + sell) → long liquidation → short liquidation → protocol reward claims
 *
 * Run:
 *   surfpool start --network mainnet --no-tui
 *   cd packages/sdk && npx tsx tests/test_e2e.ts
 */

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  VersionedTransaction,
  LAMPORTS_PER_SOL,
} from '@solana/web3.js'
import {
  getTokens,
  getToken,
  getMessages,
  getVault,
  getVaultForWallet,
  getVaultWalletLink,
  buildBuyTransaction,
  buildDirectBuyTransaction,
  buildSellTransaction,
  buildCreateTokenTransaction,
  buildMigrateTransaction,
  buildOpenLongTransaction,
  buildCloseLongTransaction,
  buildLiquidateLongTransaction,
  buildOpenShortTransaction,
  buildCloseShortTransaction,
  buildLiquidateShortTransaction,
  buildClaimProtocolRewardsTransaction,
  getUserStats,
  getProtocolTreasuryState,
  getTreasuryState,
  buildCreateVaultTransaction,
  buildDepositVaultTransaction,
  buildWithdrawVaultTransaction,
  buildWithdrawTokensTransaction,
  buildLinkWalletTransaction,
  buildUnlinkWalletTransaction,
  buildHarvestFeesTransaction,
  buildAdvanceProtocolEpochTransaction,
  confirmTransaction,
  createEphemeralAgent,
  getTokenMetadata,
  getBuyQuote,
  getSellQuote,
  getBorrowQuote,
  getLendingInfo,
  getPosition,
  getTorchVaultPda,
  getBondingCurvePda,
  getProtocolTreasuryPda,
  getTokenTreasuryPda,
  getTreasurySolVaultPda,
  getTreasuryTokenAccount,
  getDeepPoolAccounts,
  PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
} from '../src/index'
import { fetchTokenRaw } from '../src/tokens'
import { getAssociatedTokenAddressSync } from '@solana/spl-token'
import * as fs from 'fs'
import * as path from 'path'
import * as os from 'os'

// ============================================================================
// Config
// ============================================================================

const RPC_URL = 'http://localhost:8899'
const WALLET_PATH = path.join(os.homedir(), '.config/solana/id.json')

const loadWallet = (): Keypair => {
  const raw = JSON.parse(fs.readFileSync(WALLET_PATH, 'utf-8'))
  return Keypair.fromSecretKey(Uint8Array.from(raw))
}

const log = (msg: string) => {
  const ts = new Date().toISOString().substr(11, 8)
  console.log(`[${ts}] ${msg}`)
}

// Test-local helper — reads the vault's Token-2022 balance for a given mint.
// Composes public SDK helpers (getTorchVaultPda + getAssociatedTokenAddressSync).
const getVaultTokenBalance = async (
  connection: Connection,
  mint: string,
  vaultCreator: PublicKey,
): Promise<number> => {
  const [vaultPda] = getTorchVaultPda(vaultCreator)
  const ata = getAssociatedTokenAddressSync(
    new PublicKey(mint),
    vaultPda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  const bal = await connection.getTokenAccountBalance(ata)
  return Number(bal.value.amount)
}

const signAndSend = async (
  connection: Connection,
  wallet: Keypair,
  tx: Transaction | VersionedTransaction,
  quiet = false,
): Promise<string> => {
  if (tx instanceof VersionedTransaction) {
    tx.sign([wallet])
    const raw = tx.serialize()
    if (!quiet) log(`    tx size: ${raw.length}/1232 bytes`)
    const sig = await connection.sendRawTransaction(raw, {
      skipPreflight: false,
      preflightCommitment: 'confirmed',
    })
    await connection.confirmTransaction(sig, 'confirmed')
    return sig
  }
  tx.partialSign(wallet)
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
  console.log('SDK E2E TEST — Surfpool Mainnet Fork')
  console.log('='.repeat(60))

  const connection = new Connection(RPC_URL, 'confirmed')
  const funder = loadWallet()

  // Use a fresh wallet so vault is always created with the current layout
  // (mainnet fork may have stale vaults from prior program versions)
  const wallet = Keypair.generate()
  const walletAddr = wallet.publicKey.toBase58()
  // [V21] via_vault positions are owned by the TorchVault PDA, not the wallet
  // (position PDAs derive from the vault key). Every leverage position in this
  // test is opened via_vault (vault: walletAddr), so verify with the vault owner.
  const vaultOwner = getTorchVaultPda(wallet.publicKey)[0].toBase58()

  log(`Funder: ${funder.publicKey.toBase58()}`)
  log(`Test wallet: ${walletAddr} (fresh)`)
  const funderBal = await connection.getBalance(funder.publicKey)
  log(`Funder balance: ${funderBal / LAMPORTS_PER_SOL} SOL`)

  // Fund the test wallet
  const fundTx = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: funder.publicKey,
      toPubkey: wallet.publicKey,
      lamports: 800 * LAMPORTS_PER_SOL,
    }),
  )
  const { blockhash: fundBh } = await connection.getLatestBlockhash()
  fundTx.recentBlockhash = fundBh
  fundTx.feePayer = funder.publicKey
  fundTx.partialSign(funder)
  const fundSig = await connection.sendRawTransaction(fundTx.serialize())
  await connection.confirmTransaction(fundSig, 'confirmed')

  const balance = await connection.getBalance(wallet.publicKey)
  log(`Balance: ${balance / LAMPORTS_PER_SOL} SOL`)

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

  // ------------------------------------------------------------------
  // 1. Create Token
  // ------------------------------------------------------------------
  log('\n[1] Create Token')
  let mint: string
  try {
    const result = await buildCreateTokenTransaction(connection, {
      creator: walletAddr,
      name: 'SDK Test Token',
      symbol: 'SDKTEST',
      metadata_uri: 'https://example.com/test.json',
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    mint = result.mint.toBase58()
    ok('buildCreateTokenTransaction', `mint=${mint.slice(0, 8)}... sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('buildCreateTokenTransaction', e)
    console.error('Cannot continue without token. Exiting.')
    process.exit(1)
  }

  // V29: Verify on-chain Token-2022 metadata
  try {
    const metadata = await getTokenMetadata(connection, mint)
    if (!metadata) {
      fail('Token metadata', 'metadata is null')
    } else {
      const checks = [
        { field: 'name', expected: 'SDK Test Token', actual: metadata.name },
        { field: 'symbol', expected: 'SDKTEST', actual: metadata.symbol },
        { field: 'uri', expected: 'https://example.com/test.json', actual: metadata.uri },
      ]
      for (const c of checks) {
        if (c.actual === c.expected) {
          ok(`Token metadata ${c.field}`, `"${c.actual}"`)
        } else {
          fail(`Token metadata ${c.field}`, `expected "${c.expected}", got "${c.actual}"`)
        }
      }
    }
  } catch (e: any) {
    fail('Token metadata read', e)
  }

  // ------------------------------------------------------------------
  // 2. Create Vault
  // ------------------------------------------------------------------
  log('\n[2] Create Vault')
  try {
    const result = await buildCreateVaultTransaction(connection, {
      creator: walletAddr,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    ok('buildCreateVaultTransaction', `sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('buildCreateVaultTransaction', e)
  }

  // ------------------------------------------------------------------
  // 3. Deposit into Vault
  // ------------------------------------------------------------------
  log('\n[3] Deposit into Vault')
  try {
    const result = await buildDepositVaultTransaction(connection, {
      depositor: walletAddr,
      vault_creator: walletAddr,
      amount_sol: 5 * LAMPORTS_PER_SOL,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    ok('buildDepositVaultTransaction', `sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('buildDepositVaultTransaction', e)
  }

  // ------------------------------------------------------------------
  // 4. Query Vault
  // ------------------------------------------------------------------
  log('\n[4] Query Vault')
  try {
    const vault = await getVault(connection, walletAddr)
    if (!vault) throw new Error('Vault not found')
    if (vault.sol_balance < 4.9) throw new Error(`Vault balance too low: ${vault.sol_balance}`)
    if (vault.linked_wallets < 1) throw new Error(`No linked wallets: ${vault.linked_wallets}`)
    ok(
      'getVault',
      `balance=${vault.sol_balance.toFixed(2)} SOL linked_wallets=${vault.linked_wallets}`,
    )

    // Also test getVaultForWallet (creator is auto-linked)
    const vaultByWallet = await getVaultForWallet(connection, walletAddr)
    if (!vaultByWallet) throw new Error('getVaultForWallet returned null')
    ok('getVaultForWallet', `address=${vaultByWallet.address.slice(0, 8)}...`)

    // Also test getVaultWalletLink
    const link = await getVaultWalletLink(connection, walletAddr)
    if (!link) throw new Error('getVaultWalletLink returned null')
    ok('getVaultWalletLink', `vault=${link.vault.slice(0, 8)}...`)
  } catch (e: any) {
    fail('query vault', e)
  }

  // ------------------------------------------------------------------
  // 5. Get Token
  // ------------------------------------------------------------------
  log('\n[5] Get Token')
  try {
    const detail = await getToken(connection, mint)
    if (detail.name !== 'SDK Test Token') throw new Error(`Wrong name: ${detail.name}`)
    if (detail.symbol !== 'SDKTEST') throw new Error(`Wrong symbol: ${detail.symbol}`)
    if (detail.status !== 'bonding') throw new Error(`Wrong status: ${detail.status}`)
    ok(
      'getToken',
      `name=${detail.name} status=${detail.status} progress=${detail.progress_percent.toFixed(1)}%`,
    )
  } catch (e: any) {
    fail('getToken', e)
  }

  // ------------------------------------------------------------------
  // 6. List Tokens
  // ------------------------------------------------------------------
  log('\n[6] List Tokens')
  try {
    // Diagnostic: compare getProgramAccounts visibility vs getAccountInfo.
    // On surfpool, getProgramAccounts may lag for freshly-created accounts.
    const raw = await connection.getProgramAccounts(PROGRAM_ID, {
      filters: [{ memcmp: { offset: 0, bytes: '4y6pru6YvC7' } }],
    })
    const rawMints = raw.map((a) => a.pubkey.toBase58())
    log(
      `  diag: raw getProgramAccounts count=${raw.length} contains_test_pda=${rawMints.includes(
        getBondingCurvePda(new PublicKey(mint))[0].toBase58(),
      )}`,
    )

    const result = await getTokens(connection, { status: 'bonding', limit: 10 })
    log(`  diag: getTokens total=${result.total} returned=${result.tokens.length}`)
    log(`  diag: returned mints=${result.tokens.map((t) => t.mint).join(',') || '<none>'}`)
    log(`  diag: looking for=${mint}`)

    const found = result.tokens.some((t) => t.mint === mint)
    if (!found) throw new Error('Newly created token not found in list')
    ok('getTokens', `total=${result.total} found_new_token=true`)
  } catch (e: any) {
    fail('getTokens', e)
  }

  // ------------------------------------------------------------------
  // Bonding Curve Quotes — getBuyQuote / getSellQuote (pre-migration)
  // ------------------------------------------------------------------
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
      `  │    Protocol fee:    ${(buyQuote.protocol_fee_sol / 1e9).toFixed(6).padStart(15)} SOL     │`,
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

  // ------------------------------------------------------------------
  // 7. Buy Token (direct — no vault, human use)
  // ------------------------------------------------------------------
  log('\n[7] Buy Token (direct)')
  let buySig: string | undefined
  try {
    const result = await buildDirectBuyTransaction(connection, {
      mint,
      buyer: walletAddr,
      amount_sol: 100_000_000, // 0.1 SOL
      slippage_bps: 500,
    })
    buySig = await signAndSend(connection, wallet, result.transaction)
    ok('buildDirectBuyTransaction', `${result.message} sig=${buySig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('buildDirectBuyTransaction', e)
  }

  // ------------------------------------------------------------------
  // 8. Buy Token (via vault)
  // ------------------------------------------------------------------
  log('\n[8] Buy Token (via vault)')
  try {
    const vaultBefore = await getVault(connection, walletAddr)
    // V27: 2 SOL at initial price would yield ~20M tokens (near 2% wallet cap).
    // 0.5 SOL yields ~5M tokens — under cap and enough for borrow tests.
    const result = await buildBuyTransaction(connection, {
      mint,
      buyer: walletAddr,
      amount_sol: 500_000_000, // 0.5 SOL (V27: stays under 2% wallet cap)
      slippage_bps: 500,
      // No vote — wallet already voted on direct buy above
      vault: walletAddr,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    const vaultAfter = await getVault(connection, walletAddr)
    const spent = (vaultBefore?.sol_balance || 0) - (vaultAfter?.sol_balance || 0)
    ok(
      'buildBuyTransaction (vault)',
      `${result.message} vault_spent=${spent.toFixed(4)} SOL sig=${sig.slice(0, 8)}...`,
    )
  } catch (e: any) {
    fail('buildBuyTransaction (vault)', e)
  }

  // ------------------------------------------------------------------
  // 9. Ephemeral Agent — Link + Vault Buy + Unlink
  // ------------------------------------------------------------------
  log('\n[9] Ephemeral Agent (createEphemeralAgent)')
  const agent = createEphemeralAgent()
  log(`  Ephemeral key: ${agent.publicKey.slice(0, 12)}... (in-memory only)`)
  try {
    // Fund agent for tx fees only (~0.01 SOL gas)
    const fundTx = new Transaction().add(
      SystemProgram.transfer({
        fromPubkey: wallet.publicKey,
        toPubkey: agent.keypair.publicKey,
        lamports: 0.05 * LAMPORTS_PER_SOL,
      }),
    )
    const { blockhash: fBh } = await connection.getLatestBlockhash()
    fundTx.recentBlockhash = fBh
    fundTx.feePayer = wallet.publicKey
    await signAndSend(connection, wallet, fundTx, true)

    // Authority links ephemeral wallet to vault
    const linkResult = await buildLinkWalletTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      wallet_to_link: agent.publicKey,
    })
    const linkSig = await signAndSend(connection, wallet, linkResult.transaction)
    ok('link ephemeral agent', `sig=${linkSig.slice(0, 8)}...`)

    // Agent buys via vault — tokens go to vault ATA, SOL from vault
    const buyResult = await buildBuyTransaction(connection, {
      mint,
      buyer: agent.publicKey,
      amount_sol: 50_000_000, // 0.05 SOL
      slippage_bps: 500,
      vault: walletAddr,
    })
    const signedBuyTx = agent.sign(buyResult.transaction)
    const buySig2 = await connection.sendRawTransaction(signedBuyTx.serialize(), {
      skipPreflight: false,
      preflightCommitment: 'confirmed',
    })
    await connection.confirmTransaction(buySig2, 'confirmed')
    ok('ephemeral agent vault buy', `${buyResult.message} sig=${buySig2.slice(0, 8)}...`)

    // Authority unlinks ephemeral wallet — keys are now worthless
    const unlinkResult = await buildUnlinkWalletTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      wallet_to_unlink: agent.publicKey,
    })
    const unlinkSig = await signAndSend(connection, wallet, unlinkResult.transaction)
    ok('unlink ephemeral agent', `sig=${unlinkSig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('ephemeral agent lifecycle', e)
  }

  // ------------------------------------------------------------------
  // 10. Withdraw from Vault
  // ------------------------------------------------------------------
  log('\n[10] Withdraw from Vault')
  try {
    const vaultBefore = await getVault(connection, walletAddr)
    const withdrawAmount = Math.floor((vaultBefore?.sol_balance || 0) * LAMPORTS_PER_SOL * 0.5)
    const result = await buildWithdrawVaultTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      amount_sol: withdrawAmount,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    const vaultAfter = await getVault(connection, walletAddr)
    ok(
      'buildWithdrawVaultTransaction',
      `withdrew=${(withdrawAmount / LAMPORTS_PER_SOL).toFixed(2)} SOL remaining=${vaultAfter?.sol_balance.toFixed(2)} SOL sig=${sig.slice(0, 8)}...`,
    )
  } catch (e: any) {
    fail('buildWithdrawVaultTransaction', e)
  }

  // ------------------------------------------------------------------
  // 11. Sell Token (via vault)
  // ------------------------------------------------------------------
  log('\n[11] Sell Token (via vault — tokens from vault ATA, SOL to vault)')
  try {
    const vaultBefore = await getVault(connection, walletAddr)
    // Sell 10000 tokens (10000 * 1e6 base units)
    const result = await buildSellTransaction(connection, {
      mint,
      seller: walletAddr,
      amount_tokens: 10_000_000_000, // 10000 tokens
      slippage_bps: 500,
      vault: walletAddr,
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    const vaultAfter = await getVault(connection, walletAddr)
    const received = (vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)
    ok(
      'buildSellTransaction (vault)',
      `${result.message} vault_received=${received.toFixed(6)} SOL sig=${sig.slice(0, 8)}...`,
    )
  } catch (e: any) {
    fail('buildSellTransaction (vault)', e)
  }

  // ------------------------------------------------------------------
  // 11b. Withdraw Tokens from Vault (escape hatch)
  // ------------------------------------------------------------------
  log('\n[11b] Withdraw Tokens from Vault')
  try {
    const result = await buildWithdrawTokensTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      mint,
      destination: walletAddr,
      amount: 500_000_000, // 500 tokens
    })
    const sig = await signAndSend(connection, wallet, result.transaction)
    ok('buildWithdrawTokensTransaction', `${result.message} sig=${sig.slice(0, 8)}...`)
  } catch (e: any) {
    fail('buildWithdrawTokensTransaction', e)
  }

  // ------------------------------------------------------------------
  // 12. Star Token — [V21] REMOVED. The star/creator-reward feature was ripped
  // out; creator monetization is the creator token, not community-token+stars.
  // ------------------------------------------------------------------

  // ------------------------------------------------------------------
  // 13. Get Messages
  // ------------------------------------------------------------------
  log('\n[13] Get Messages')
  try {
    // Wait a moment for the tx to be indexed
    await new Promise((r) => setTimeout(r, 1000))
    const result = await getMessages(connection, mint, 10)
    ok('getMessages', `count=${result.messages.length}`)
  } catch (e: any) {
    fail('getMessages', e)
  }

  // ------------------------------------------------------------------
  // 14. Confirm Transaction (SAID)
  // ------------------------------------------------------------------
  log('\n[14] Confirm Transaction')
  if (buySig) {
    try {
      const result = await confirmTransaction(connection, buySig, walletAddr)
      if (!result.confirmed) throw new Error('Not confirmed')
      ok('confirmTransaction', `event=${result.event_type}`)
    } catch (e: any) {
      fail('confirmTransaction', e)
    }
  } else {
    fail('confirmTransaction', { message: 'No buy sig to confirm' })
  }

  // ------------------------------------------------------------------
  // 15. Bond to Completion + Migrate + Borrow + Repay
  // ------------------------------------------------------------------
  log('\n[15] Full Lifecycle: Bond → Migrate → Borrow → Repay')
  log('  Bonding to 200 SOL using multiple wallets (2% wallet cap)...')

  // V27: With IVS=75 SOL and IVT=756.25M, max buy at initial price ≈ 2 SOL
  // before hitting the 2% wallet cap (20M tokens). Use 1.5 SOL buys for faster bonding.
  const NUM_BUYERS = 200
  const BUY_AMOUNT = Math.floor(1.5 * LAMPORTS_PER_SOL) // 1.5 SOL per buy
  const buyers: Keypair[] = []
  for (let i = 0; i < NUM_BUYERS; i++) buyers.push(Keypair.generate())

  // Fund in batches of 20
  for (let i = 0; i < buyers.length; i += 20) {
    const batch = buyers.slice(i, i + 20)
    const fundTx = new Transaction()
    for (const b of batch) {
      fundTx.add(
        SystemProgram.transfer({
          fromPubkey: wallet.publicKey,
          toPubkey: b.publicKey,
          lamports: BUY_AMOUNT + Math.floor(0.05 * LAMPORTS_PER_SOL),
        }),
      )
    }
    const { blockhash: fBh } = await connection.getLatestBlockhash()
    fundTx.recentBlockhash = fBh
    fundTx.feePayer = wallet.publicKey
    await signAndSend(connection, wallet, fundTx, true)
  }
  log(`  Funded ${buyers.length} wallets with ${BUY_AMOUNT / LAMPORTS_PER_SOL} SOL each`)

  // Buy until bonding completes
  let bondingComplete = false
  let buyCount = 0
  for (const buyer of buyers) {
    if (bondingComplete) break
    try {
      const result = await buildDirectBuyTransaction(connection, {
        mint,
        buyer: buyer.publicKey.toBase58(),
        amount_sol: BUY_AMOUNT,
        slippage_bps: 1000,
      })
      await signAndSend(connection, buyer, result.transaction, true)
      buyCount++

      if (buyCount % 50 === 0) {
        const detail = await getToken(connection, mint)
        log(
          `  Buy ${buyCount}: ${detail.progress_percent.toFixed(1)}% (${detail.sol_raised.toFixed(1)} SOL)`,
        )
        if (detail.status !== 'bonding') bondingComplete = true
      }
    } catch (e: any) {
      if (
        e.message?.includes('Bonding curve complete') ||
        e.message?.includes('bonding_complete') ||
        e.message?.includes('BondingComplete')
      ) {
        bondingComplete = true
      } else if (e.message?.includes('Migrated tokens require vault-based trading')) {
        // NOOP
      } else {
        // Skip individual failures (e.g. wallet cap edge cases)
        log(`  Buy ${buyCount + 1} skipped: ${e.message?.substring(0, 80)}`)
      }
    }
  }
  // Check final status
  try {
    const detail = await getToken(connection, mint)
    if (detail.status !== 'bonding') bondingComplete = true
    log(
      `  Final: ${detail.progress_percent.toFixed(1)}% (${detail.sol_raised.toFixed(1)} SOL) status=${detail.status}`,
    )
  } catch {
    /* ignore */
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
    ok('bonding complete', `after ${buyCount} buys`)
  } else {
    fail('bonding', { message: `Only ${buyCount} buys, not complete` })
  }

  // Migrate to DeepPool via SDK
  if (bondingComplete) {
    log('  Migrating to DeepPool (via SDK)...')
    try {
      // Snapshot bonding curve state before migration for price verification
      const mintPk = new PublicKey(mint)
      const snap = await fetchTokenRaw(connection, mintPk)
      if (!snap) throw new Error('token not found pre-migration')
      const bcData = snap.bondingCurve

      // Auto-migration bundled with last buy?
      if (bcData.migrated) {
        ok('migrate to DEX', 'auto-migrated with last buy')
      } else {
        // Fallback: separate migration call
        const migrateResult = await buildMigrateTransaction(connection, {
          mint,
          payer: walletAddr,
        })
        await signAndSend(connection, wallet, migrateResult.transaction)
        ok('migrate to DEX', 'DeepPool created (fallback — separate migration call)')
      }

      // Derive DeepPool addresses for post-migration verification
      const deepPool = getDeepPoolAccounts(mintPk)
      const DEEP_POOL_STATE_LEN = 129

      // V27: Post-migration token distribution breakdown
      try {
        const postMigData = await fetchTokenRaw(connection, mintPk)
        const bc = postMigData!.bondingCurve
        const tr = postMigData!.treasury!

        const TOTAL_SUPPLY = 1_000_000_000 // 1B tokens (display units)
        const TREASURY_LOCK = 300_000_000 // 300M locked in treasury lock PDA
        const CURVE_SUPPLY = 700_000_000 // 700M for curve + pool
        const poolTokenBalPost = await connection.getTokenAccountBalance(deepPool.tokenVault)
        const poolTokens = Number(poolTokenBalPost.value.amount) / 1e6
        // V20: vote vault removed (always 0), zero-burn migration (no excess tokens burned).
        // tokensSold reduces to CURVE_SUPPLY - poolTokens.
        const tokensSold = CURVE_SUPPLY - poolTokens
        // [V21] Treasury SOL lives in the System-owned treasury_sol_vault PDA.
        const treasurySolVaultInfo = await connection.getAccountInfo(
          getTreasurySolVaultPda(new PublicKey(mint))[0],
        )
        const treasurySol = (treasurySolVaultInfo?.lamports ?? 0) / LAMPORTS_PER_SOL
        const poolAcctInfo = await connection.getAccountInfo(deepPool.pool)
        const rentExempt = await connection.getMinimumBalanceForRentExemption(DEEP_POOL_STATE_LEN)
        const poolSol2 = (poolAcctInfo!.lamports - rentExempt) / LAMPORTS_PER_SOL
        const baselineSol = Number(tr.baseline_sol_reserves.toString()) / LAMPORTS_PER_SOL
        const baselineTokens = Number(tr.baseline_token_reserves.toString()) / 1e6

        // V27: Determine initial virtual reserves for this token's tier
        const bondingTarget = Number(bc.bonding_target.toString())
        let ivs = 30 // legacy default
        let ivt = 107_300_000 // legacy default
        if (bondingTarget === 50_000_000_000) {
          ivs = 18.75
          ivt = 756_250_000
        } else if (bondingTarget === 100_000_000_000) {
          ivs = 37.5
          ivt = 756_250_000
        } else if (bondingTarget === 200_000_000_000) {
          ivs = 75
          ivt = 756_250_000
        }

        const entryPrice = ivs / ivt
        const exitPrice = poolSol2 / poolTokens
        const multiplier = exitPrice / entryPrice
        const initialMcSol = TOTAL_SUPPLY * entryPrice
        const finalMcSol = TOTAL_SUPPLY * exitPrice

        log(`\n  ┌─── V31 Post-Migration Token Distribution ─────────────────┐`)
        log(`  │  Total Supply:     ${TOTAL_SUPPLY.toLocaleString().padStart(15)} tokens  │`)
        log(`  │  Treasury Lock:    ${TREASURY_LOCK.toLocaleString().padStart(15)} tokens  │`)
        log(`  │  Tokens Sold:      ${tokensSold.toFixed(0).padStart(15)} tokens  │`)
        log(`  │  Pool Tokens:      ${poolTokens.toFixed(0).padStart(15)} tokens  │`)
        log(`  ├────────────────────────────────────────────────────────────┤`)
        log(`  │  Pool SOL:         ${poolSol2.toFixed(4).padStart(15)} SOL     │`)
        log(`  │  Treasury SOL:     ${treasurySol.toFixed(4).padStart(15)} SOL     │`)
        log(`  │  Baseline SOL:     ${baselineSol.toFixed(4).padStart(15)} SOL     │`)
        log(`  │  Baseline Tokens:  ${baselineTokens.toFixed(0).padStart(15)} tokens  │`)
        log(`  ├────────────────────────────────────────────────────────────┤`)
        log(`  │  Entry Price:      ${entryPrice.toExponential(4).padStart(15)} SOL/tok │`)
        log(`  │  Exit Price:       ${exitPrice.toExponential(4).padStart(15)} SOL/tok │`)
        log(`  │  Multiplier:       ${multiplier.toFixed(1).padStart(15)}x        │`)
        log(`  │  Initial MC:       ${initialMcSol.toFixed(2).padStart(15)} SOL     │`)
        log(`  │  Final MC:         ${finalMcSol.toFixed(2).padStart(15)} SOL     │`)
        log(
          `  │  Sold %:           ${((tokensSold / CURVE_SUPPLY) * 100).toFixed(1).padStart(14)}%         │`,
        )
        log(`  └────────────────────────────────────────────────────────────┘`)
      } catch {
        /* non-critical */
      }

      // Time travel 100 slots (DeepPool has no open_time gate, but keeps parity with test flow)
      log('  Time traveling 100 slots...')
      const slotAfterMigrate = await connection.getSlot()
      await fetch('http://127.0.0.1:8899', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 1,
          method: 'surfnet_timeTravel',
          params: [{ absoluteSlot: slotAfterMigrate + 100 }],
        }),
      })
      await new Promise((r) => setTimeout(r, 500))

      // Verify pool price matches bonding curve exit price
      log('  Verifying pool price matches bonding curve exit price...')
      const virtualSol = Number(bcData.virtual_sol_reserves.toString())
      const virtualTokens = Number(bcData.virtual_token_reserves.toString())
      const curvePrice = virtualSol / virtualTokens

      // Read DeepPool reserves
      const poolAcct = await connection.getAccountInfo(deepPool.pool)
      const poolRentExempt = await connection.getMinimumBalanceForRentExemption(DEEP_POOL_STATE_LEN)
      const poolSol = poolAcct!.lamports - poolRentExempt
      const poolTokenBal = await connection.getTokenAccountBalance(deepPool.tokenVault)
      const poolTokens = Number(poolTokenBal.value.amount)
      const poolPrice = poolSol / poolTokens

      const priceRatio = poolPrice / curvePrice
      log(`    Curve exit price:  ${curvePrice.toFixed(12)} SOL/token`)
      log(`    Pool open price:   ${poolPrice.toFixed(12)} SOL/token`)
      log(`    Ratio (pool/curve): ${priceRatio.toFixed(4)} (should be ~1.0)`)
      log(
        `    Pool SOL: ${(poolSol / LAMPORTS_PER_SOL).toFixed(4)}, Pool tokens: ${(poolTokens / 1e6).toFixed(0)}`,
      )

      if (priceRatio > 0.9 && priceRatio < 1.1) {
        ok('Pool price check', `ratio=${priceRatio.toFixed(4)} — within 10% of curve price`)
      } else {
        fail('Pool price check', {
          message: `ratio=${priceRatio.toFixed(4)} — price mismatch! Expected ~1.0`,
        })
      }

      // ------------------------------------------------------------------
      // DEX Quotes — getBuyQuote / getSellQuote on migrated token
      // ------------------------------------------------------------------
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
        log(
          `  │    Min output:      ${(buyQuote.min_output_tokens / 1e6).toFixed(2).padStart(15)}         │`,
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
        log(
          `  │    Min output:      ${(sellQuote.min_output_sol / 1e9).toFixed(6).padStart(15)} SOL     │`,
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
      // POST-MIGRATION TRADING — Pool Depth Growth from Auto-Compounding
      // ==================================================================
      log('\n  Post-migration trading: 10 wash-traders × 5 cycles to grow pool depth...')
      try {
        // Top up vault for post-migration trading (bonding drained it)
        const topUpResult = await buildDepositVaultTransaction(connection, {
          depositor: walletAddr,
          vault_creator: walletAddr,
          amount_sol: 200 * LAMPORTS_PER_SOL,
        })
        await signAndSend(connection, wallet, topUpResult.transaction, true)
        log('  Deposited 200 SOL to vault for trading')

        // Spawn 10 ephemeral wash-traders, link to main vault
        const NUM_TRADERS = 10
        const CYCLES_PER_TRADER = 5
        const BUY_SOL = 2 * LAMPORTS_PER_SOL
        const traders: Keypair[] = []

        for (let i = 0; i < NUM_TRADERS; i++) {
          const trader = Keypair.generate()
          // Fund for tx fees
          const fundTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: trader.publicKey,
              lamports: Math.floor(0.1 * LAMPORTS_PER_SOL),
            }),
          )
          const { blockhash: bh } = await connection.getLatestBlockhash()
          fundTx.recentBlockhash = bh
          fundTx.feePayer = wallet.publicKey
          await signAndSend(connection, wallet, fundTx, true)

          // Link to main vault
          const linkResult = await buildLinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_link: trader.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, linkResult.transaction, true)
          traders.push(trader)
        }
        log(`  Funded & linked ${NUM_TRADERS} wash-traders`)

        // Snapshot pool depth before trading
        const dpSnap = getDeepPoolAccounts(mintPk)
        const snapAcct = await connection.getAccountInfo(dpSnap.pool)
        const snapRent = await connection.getMinimumBalanceForRentExemption(DEEP_POOL_STATE_LEN)
        const poolSolBefore = (snapAcct!.lamports - snapRent) / LAMPORTS_PER_SOL

        const T2022 = new PublicKey('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb')
        const { getTorchVaultPda: gvpTrade } = require('../src/program')
        const [mainVaultPda] = gvpTrade(wallet.publicKey)

        // Each trader does CYCLES_PER_TRADER buy/sell round-trips
        for (let c = 0; c < CYCLES_PER_TRADER; c++) {
          for (const trader of traders) {
            const traderAddr = trader.publicKey.toBase58()

            // Snapshot vault token balance before buy
            const { getAssociatedTokenAddressSync: gataT } = require('@solana/spl-token')
            const vaultAta = gataT(new PublicKey(mint), mainVaultPda, true, T2022)
            let tokensBefore = 0
            try {
              const balBefore = await connection.getTokenAccountBalance(vaultAta)
              tokensBefore = Number(balBefore.value.amount)
            } catch {
              /* ATA may not exist yet */
            }

            // Buy via vault
            const buyResult = await buildBuyTransaction(connection, {
              mint,
              buyer: traderAddr,
              amount_sol: BUY_SOL,
              slippage_bps: 500,
              vault: walletAddr,
            })
            await signAndSend(connection, trader, buyResult.transaction, true)

            // Sell back exactly what this buy produced (delta)
            const balAfter = await connection.getTokenAccountBalance(vaultAta)
            const tokensReceived = Number(balAfter.value.amount) - tokensBefore

            if (tokensReceived > 0) {
              const sellResult = await buildSellTransaction(connection, {
                mint,
                seller: traderAddr,
                amount_tokens: tokensReceived,
                slippage_bps: 500,
                vault: walletAddr,
              })
              await signAndSend(connection, trader, sellResult.transaction, true)
            }
          }
          log(`  Cycle ${c + 1}/${CYCLES_PER_TRADER} complete`)
        }

        // Unlink traders
        for (const trader of traders) {
          const unlinkResult = await buildUnlinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_unlink: trader.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, unlinkResult.transaction, true)
        }

        // Snapshot pool depth after trading
        const snapAcct2 = await connection.getAccountInfo(dpSnap.pool)
        const poolSolAfter = (snapAcct2!.lamports - snapRent) / LAMPORTS_PER_SOL
        const depthGrowth = poolSolAfter - poolSolBefore
        const growthPct = ((depthGrowth / poolSolBefore) * 100).toFixed(2)

        log(`\n  ┌─── DeepPool Depth Growth (Auto-Compounding Fees) ─────────┐`)
        log(`  │  Pool SOL before:     ${poolSolBefore.toFixed(4).padStart(12)} SOL          │`)
        log(`  │  Pool SOL after:      ${poolSolAfter.toFixed(4).padStart(12)} SOL          │`)
        log(
          `  │  Depth growth:        ${depthGrowth.toFixed(4).padStart(13)} SOL (+${growthPct}%) │`,
        )
        log(
          `  │  Volume:              ${((NUM_TRADERS * CYCLES_PER_TRADER * BUY_SOL * 2) / LAMPORTS_PER_SOL).toFixed(0).padStart(13)} SOL          │`,
        )
        log(`  └────────────────────────────────────────────────────────────┘`)

        ok(
          'pool depth growth',
          `+${depthGrowth.toFixed(4)} SOL (+${growthPct}%) from ${NUM_TRADERS}×${CYCLES_PER_TRADER} wash trades`,
        )
      } catch (e: any) {
        fail('pool depth growth', e)
      }

      // ==================================================================
      // MARGIN STRESS TESTS (Lending + Short Selling)
      // ==================================================================

      // Stock vault with tokens for margin tests (wash trades leave vault empty on DeepPool)
      try {
        const stockResult = await buildBuyTransaction(connection, {
          mint,
          buyer: walletAddr,
          amount_sol: 10 * LAMPORTS_PER_SOL,
          slippage_bps: 500,
          vault: walletAddr,
        })
        await signAndSend(connection, wallet, stockResult.transaction, true)
        log('  Bought 10 SOL of tokens into vault for margin tests')
      } catch (e: any) {
        log(`  Warning: margin stock buy failed: ${e.message?.substring(0, 60)}`)
      }

      // ------------------------------------------------------------------
      // M1. getLendingInfo — verify pool params after migration [V21]
      // ------------------------------------------------------------------
      log('\n[M1] getLendingInfo — pool parameters')
      try {
        const info = await getLendingInfo(connection, mint)
        log(
          `  interest_rate=${info.interest_rate_bps}bps, max_ltv=${info.max_ltv_bps}bps, liq_threshold=${info.liquidation_threshold_bps}bps`,
        )
        log(
          `  utilization_cap=${info.utilization_cap_bps}bps, lending_enabled=${info.lending_enabled}, short_enabled=${info.short_selling_enabled}`,
        )
        log(
          `  treasury_sol_vault=${(info.treasury_sol_vault_lamports / LAMPORTS_PER_SOL).toFixed(4)} SOL, active_longs=${info.active_longs}, active_shorts=${info.active_shorts}`,
        )
        if (
          info.interest_rate_bps > 0 &&
          info.max_ltv_bps > 0 &&
          info.max_ltv_bps <= 9000 &&
          info.liquidation_threshold_bps > 0
        ) {
          ok(
            'getLendingInfo',
            `params present, vault=${(info.treasury_sol_vault_lamports / LAMPORTS_PER_SOL).toFixed(4)} SOL`,
          )
        } else {
          fail('getLendingInfo', { message: 'unexpected lending params' })
        }
      } catch (e: any) {
        fail('getLendingInfo', e)
      }

      // ------------------------------------------------------------------
      // M1b. getTreasuryState — verify per-token Treasury reader [V21]
      // ------------------------------------------------------------------
      log('\n[M1b] getTreasuryState — per-token treasury reader')
      try {
        const ts = await getTreasuryState(connection, mint)
        if (!ts) throw new Error('Treasury not found for migrated token')
        log(
          `  address=${ts.address.slice(0, 16)}...  vault_sol=${ts.treasury_sol_vault_sol.toFixed(4)} SOL  lending=${ts.lending_enabled}`,
        )
        log(
          `  baseline_initialized=${ts.baseline_initialized}  baseline_sol=${(ts.baseline_sol_reserves / LAMPORTS_PER_SOL).toFixed(2)}  baseline_tokens=${(ts.baseline_token_reserves / 1e6).toFixed(0)}`,
        )
        if (ts.mint !== mint) {
          throw new Error(`Treasury.mint mismatch: got ${ts.mint}, expected ${mint}`)
        }
        if (!ts.baseline_initialized) {
          throw new Error('baseline_initialized should be true after migration')
        }
        ok(
          'getTreasuryState',
          `vault_sol=${ts.treasury_sol_vault_sol.toFixed(4)} SOL, baseline_initialized=${ts.baseline_initialized}`,
        )
      } catch (e: any) {
        fail('getTreasuryState', e)
      }

      // ------------------------------------------------------------------
      // M2. Open Long — post token collateral, borrow SOL (clamped) [V21]
      // ------------------------------------------------------------------
      log('\n[M2] Open Long — token collateral, auto-clamped borrow')
      let stressBorrowActive = false // track whether we have an active long for later tests

      try {
        const totalTokens = await getVaultTokenBalance(connection, mint, wallet.publicKey)
        log(`  Vault token balance: ${(totalTokens / 1e6).toFixed(0)} tokens`)

        // Use 40% as collateral (save rest for short tests).
        const collateralAmount = Math.floor(totalTokens * 0.4)
        const quote = await getBorrowQuote(connection, mint, collateralAmount)
        log(
          `  collateral_value: ${(quote.collateral_value_sol / LAMPORTS_PER_SOL).toFixed(4)} SOL, ltv_max: ${(quote.ltv_max_sol / LAMPORTS_PER_SOL).toFixed(4)}, pool_available: ${(quote.pool_available_sol / LAMPORTS_PER_SOL).toFixed(4)}, max borrow: ${(quote.max_borrow_sol / LAMPORTS_PER_SOL).toFixed(4)}`,
        )

        if (quote.max_borrow_sol < 100_000_000) {
          // MIN_BORROW_AMOUNT — V21 rejects too-small borrows.
          log('  Skipping — lending capacity too low for open long')
          ok('open long', 'skipped — lending capacity too low')
        } else {
          // V21: no borrow-size knob. Borrow = collateral × LTV, clamped to caps.
          const openResult = await buildOpenLongTransaction(connection, {
            mint,
            borrower: walletAddr,
            collateral: collateralAmount,
            vault: walletAddr,
          })
          const openSig = await signAndSend(connection, wallet, openResult.transaction)
          ok(
            'open long',
            `${openResult.message} (est borrow ~${(quote.max_borrow_sol / LAMPORTS_PER_SOL).toFixed(4)} SOL) sig=${openSig.slice(0, 8)}...`,
          )
          stressBorrowActive = true

          // ----------------------------------------------------------------
          // M3. getPosition('long') — verify position state after open
          // ----------------------------------------------------------------
          log('\n[M3] getPosition (long) — verify active position')
          let openDebt = 0
          try {
            const pos = await getPosition(connection, mint, vaultOwner, 'long', 0)
            openDebt = pos.debt_amount
            log(
              `  collateral=${(pos.collateral_amount / 1e6).toFixed(0)} tokens, debt=${(pos.debt_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, interest=${pos.accrued_interest}, health=${pos.health}`,
            )
            log(
              `  debt_value=${pos.debt_value_sol !== null ? (pos.debt_value_sol / LAMPORTS_PER_SOL).toFixed(4) + ' SOL' : 'null'}, LTV=${pos.current_ltv_bps !== null ? (pos.current_ltv_bps / 100).toFixed(1) + '%' : 'null'}`,
            )

            if (pos.health !== 'none' && pos.debt_amount > 0 && pos.collateral_amount > 0) {
              ok(
                'getPosition long (active)',
                `health=${pos.health}, LTV=${pos.current_ltv_bps !== null ? (pos.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}`,
              )
            } else {
              fail('getPosition long (active)', {
                message: `unexpected: health=${pos.health} debt=${pos.debt_amount}`,
              })
            }
          } catch (e: any) {
            fail('getPosition long (active)', e)
          }

          // ----------------------------------------------------------------
          // M4. Partial close — close 50%, verify position still active
          // ----------------------------------------------------------------
          log('\n[M4] Partial Close Long — 50% of position')
          try {
            const closeResult = await buildCloseLongTransaction(connection, {
              mint,
              borrower: walletAddr,
              repay_fraction_bps: 5000, // half
              vault: walletAddr,
            })
            const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
            ok('partial close long', `${closeResult.message} sig=${closeSig.slice(0, 8)}...`)

            const posAfter = await getPosition(connection, mint, vaultOwner, 'long', 0)
            log(
              `  After partial close: debt=${(posAfter.debt_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, health=${posAfter.health}`,
            )
            if (posAfter.debt_amount > 0 && posAfter.debt_amount < openDebt) {
              ok(
                'getPosition long (after partial close)',
                `debt reduced, health=${posAfter.health}`,
              )
            } else {
              fail('getPosition long (after partial close)', {
                message: `unexpected debt=${posAfter.debt_amount}`,
              })
            }
          } catch (e: any) {
            fail('partial close long', e)
          }

          // ----------------------------------------------------------------
          // M5. Full close — close the position
          // ----------------------------------------------------------------
          log('\n[M5] Full Close Long — close position')
          try {
            const closeResult = await buildCloseLongTransaction(connection, {
              mint,
              borrower: walletAddr,
              repay_fraction_bps: 10000, // full
              vault: walletAddr,
            })
            const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
            ok('full close long', `${closeResult.message} sig=${closeSig.slice(0, 8)}...`)
            stressBorrowActive = false

            const posAfter = await getPosition(connection, mint, vaultOwner, 'long', 0)
            if (posAfter.health === 'none' || posAfter.debt_amount === 0) {
              ok('getPosition long (after full close)', 'position closed')
            } else {
              fail('getPosition long (after full close)', {
                message: `still active: debt=${posAfter.debt_amount}`,
              })
            }
          } catch (e: any) {
            fail('full close long', e)
          }
        } // end else (max_borrow >= MIN_BORROW)
      } catch (e: any) {
        fail('open long', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // M6. Short Selling — open, verify position, partial close, full close
      // ------------------------------------------------------------------
      log('\n[M6] Short Selling Stress: Open → Verify → Partial Close → Full Close')
      try {
        // Deposit SOL to vault for short collateral
        const depositForShort = await buildDepositVaultTransaction(connection, {
          depositor: walletAddr,
          vault_creator: walletAddr,
          amount_sol: 2 * LAMPORTS_PER_SOL,
        })
        await signAndSend(connection, wallet, depositForShort.transaction, true)
        log('  Deposited 2 SOL to vault for short collateral')

        const vaultBeforeShort = await getVault(connection, walletAddr)
        const vaultSolInSol = vaultBeforeShort?.sol_balance || 0
        log(`  Vault SOL balance: ${vaultSolInSol.toFixed(4)} SOL`)

        // [V21] Post 1 SOL as collateral; borrowed tokens = collateral × LTV
        // (clamped on-chain — no tokens_to_borrow knob).
        const shortCollateral = Math.floor(1 * LAMPORTS_PER_SOL)

        if (vaultSolInSol < 1.0) {
          log('  Skipping short — vault SOL too low for 1 SOL collateral')
          ok('short selling stress', 'skipped — insufficient vault SOL')
        } else {
          // Open short
          const openResult = await buildOpenShortTransaction(connection, {
            mint,
            shorter: walletAddr,
            collateral: shortCollateral,
            vault: walletAddr,
          })
          const openSig = await signAndSend(connection, wallet, openResult.transaction)
          const vaultAfterOpen = await getVault(connection, walletAddr)
          const solSpent = (vaultBeforeShort?.sol_balance || 0) - (vaultAfterOpen?.sol_balance || 0)
          ok(
            'open short',
            `${openResult.message} collateral=${(solSpent / LAMPORTS_PER_SOL).toFixed(4)} SOL sig=${openSig.slice(0, 8)}...`,
          )

          // Verify short position via getPosition('short')
          log('\n  Verifying short position...')
          let openShortDebt = 0
          try {
            const shortPos = await getPosition(connection, mint, vaultOwner, 'short', 0)
            openShortDebt = shortPos.debt_amount
            log(
              `  collateral=${(shortPos.collateral_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, debt=${(shortPos.debt_amount / 1e6).toFixed(0)} tokens, interest=${(shortPos.accrued_interest / 1e6).toFixed(0)}`,
            )
            log(
              `  debt_value=${shortPos.debt_value_sol !== null ? (shortPos.debt_value_sol / LAMPORTS_PER_SOL).toFixed(4) + ' SOL' : 'null'}, LTV=${shortPos.current_ltv_bps !== null ? (shortPos.current_ltv_bps / 100).toFixed(1) + '%' : 'null'}, health=${shortPos.health}`,
            )

            if (
              shortPos.health !== 'none' &&
              shortPos.debt_amount > 0 &&
              shortPos.collateral_amount > 0
            ) {
              ok(
                'getPosition short (active)',
                `health=${shortPos.health}, LTV=${shortPos.current_ltv_bps !== null ? (shortPos.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}`,
              )
            } else {
              fail('getPosition short (active)', {
                message: `unexpected: health=${shortPos.health}`,
              })
            }
          } catch (e: any) {
            fail('getPosition short (active)', e)
          }

          // Partial close — buy back half the debt
          log('\n  Partial close short (50%)...')
          try {
            const closeResult = await buildCloseShortTransaction(connection, {
              mint,
              shorter: walletAddr,
              repay_fraction_bps: 5000, // half
              vault: walletAddr,
            })
            const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
            ok('partial close short', `${closeResult.message} sig=${closeSig.slice(0, 8)}...`)

            // Verify position still active with reduced debt
            const posAfter = await getPosition(connection, mint, vaultOwner, 'short', 0)
            log(
              `  After partial close: debt=${(posAfter.debt_amount / 1e6).toFixed(0)} tokens, health=${posAfter.health}`,
            )
            if (posAfter.debt_amount > 0 && posAfter.debt_amount < openShortDebt) {
              ok(
                'getPosition short (after partial close)',
                `debt reduced to ${(posAfter.debt_amount / 1e6).toFixed(0)} tokens`,
              )
            } else {
              fail('getPosition short (after partial close)', {
                message: `unexpected: debt=${posAfter.debt_amount}`,
              })
            }
          } catch (e: any) {
            fail('partial close short', e)
            if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
          }

          // Full close — repay 100%
          log('\n  Full close short...')
          try {
            const closeResult = await buildCloseShortTransaction(connection, {
              mint,
              shorter: walletAddr,
              repay_fraction_bps: 10000, // full
              vault: walletAddr,
            })
            const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
            const vaultAfterClose = await getVault(connection, walletAddr)
            const solReturned =
              (vaultAfterClose?.sol_balance || 0) - (vaultAfterOpen?.sol_balance || 0)
            ok(
              'full close short',
              `vault_sol_delta=${(solReturned / LAMPORTS_PER_SOL).toFixed(4)} SOL sig=${closeSig.slice(0, 8)}...`,
            )

            // Verify position closed
            const posAfter = await getPosition(connection, mint, vaultOwner, 'short', 0)
            if (posAfter.health === 'none' || posAfter.debt_amount === 0) {
              ok('getPosition short (after full close)', 'position closed')
            } else {
              fail('getPosition short (after full close)', {
                message: `still active: debt=${posAfter.debt_amount}`,
              })
            }
          } catch (e: any) {
            fail('full close short', e)
            if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
          }
        } // end else (vault SOL sufficient)
      } catch (e: any) {
        fail('short selling stress', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // 16. DEX Buy via buildBuyTransaction (auto-routes through vault swap)
      // ------------------------------------------------------------------
      log('\n[16] DEX Buy via buildBuyTransaction (post-migration)')
      try {
        const buyQuote = await getBuyQuote(connection, mint, 100_000_000) // 0.1 SOL
        log(
          `  Quote: ${(buyQuote.tokens_to_user / 1e6).toFixed(2)} tokens for 0.1 SOL (source=${buyQuote.source})`,
        )

        const vaultBefore = await getVault(connection, walletAddr)
        const buyResult = await buildBuyTransaction(connection, {
          mint,
          buyer: walletAddr,
          amount_sol: 100_000_000,
          slippage_bps: 500,
          vault: walletAddr,
          quote: buyQuote,
          message: 'Post-migration buy via unified flow',
        })
        const buySig = await signAndSend(connection, wallet, buyResult.transaction)
        const vaultAfter = await getVault(connection, walletAddr)
        const spent = (vaultBefore?.sol_balance || 0) - (vaultAfter?.sol_balance || 0)
        ok(
          'buildBuyTransaction (DEX)',
          `${buyResult.message} vault_spent=${spent.toFixed(4)} SOL sig=${buySig.slice(0, 8)}...`,
        )
      } catch (e: any) {
        fail('buildBuyTransaction (DEX)', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // 17. DEX Sell via buildSellTransaction (auto-routes through vault swap)
      // ------------------------------------------------------------------
      log('\n[17] DEX Sell via buildSellTransaction (post-migration)')
      try {
        // Read vault's token balance to sell a portion
        const totalTokens2 = await getVaultTokenBalance(connection, mint, wallet.publicKey)
        const sellAmount = Math.floor(totalTokens2 * 0.1) // sell 10% of vault tokens
        log(
          `  Vault token balance: ${(totalTokens2 / 1e6).toFixed(0)} tokens, selling ${(sellAmount / 1e6).toFixed(0)}`,
        )

        const sellQuote = await getSellQuote(connection, mint, sellAmount)
        log(
          `  Quote: ${(sellQuote.output_sol / 1e9).toFixed(6)} SOL for ${(sellAmount / 1e6).toFixed(0)} tokens (source=${sellQuote.source})`,
        )

        const vaultBefore = await getVault(connection, walletAddr)
        const sellResult = await buildSellTransaction(connection, {
          mint,
          seller: walletAddr,
          amount_tokens: sellAmount,
          slippage_bps: 500,
          vault: walletAddr,
          quote: sellQuote,
          message: 'Taking profits post-migration',
        })
        const sellSig = await signAndSend(connection, wallet, sellResult.transaction)
        const vaultAfter = await getVault(connection, walletAddr)
        const received = (vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)
        ok(
          'buildSellTransaction (DEX)',
          `${sellResult.message} vault_received=${received.toFixed(6)} SOL sig=${sellSig.slice(0, 8)}...`,
        )
      } catch (e: any) {
        fail('buildSellTransaction (DEX)', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }
      // ------------------------------------------------------------------
      // 18. Harvest Transfer Fees
      // ------------------------------------------------------------------
      log('\n[18] Harvest Transfer Fees')
      try {
        // The vault swap buys above generated transfer fees (1% on token transfers)
        // Snapshot treasury state before harvest
        const preHarvestData = await fetchTokenRaw(connection, new PublicKey(mint))
        // [V21] Treasury SOL lives in the System-owned treasury_sol_vault PDA.
        const treasurySolVaultPda = getTreasurySolVaultPda(new PublicKey(mint))[0]
        const preSolBalance =
          ((await connection.getAccountInfo(treasurySolVaultPda))?.lamports ?? 0) / LAMPORTS_PER_SOL
        const preHarvestedFees =
          Number(preHarvestData?.treasury?.harvested_fees?.toString() || '0') / LAMPORTS_PER_SOL

        // Read treasury token account balance (where harvested tokens actually go)
        const mintPk2 = new PublicKey(mint)
        const [treasuryPda] = getTokenTreasuryPda(mintPk2)
        const treasuryAta = getTreasuryTokenAccount(mintPk2, treasuryPda)
        let preTokenBal = 0
        try {
          const bal = await connection.getTokenAccountBalance(treasuryAta)
          preTokenBal = Number(bal.value.amount)
        } catch {
          /* ATA may not exist yet */
        }

        log(
          `  [before] treasury_sol=${preSolBalance.toFixed(4)} SOL, treasury_tokens=${(preTokenBal / 1e6).toFixed(2)}, harvested_fees=${preHarvestedFees.toFixed(6)} SOL`,
        )

        const harvestResult = await buildHarvestFeesTransaction(connection, {
          mint,
          payer: walletAddr,
        })
        const harvestSig = await signAndSend(connection, wallet, harvestResult.transaction)

        // Snapshot after harvest
        const postSolBalance =
          ((await connection.getAccountInfo(treasurySolVaultPda))?.lamports ?? 0) / LAMPORTS_PER_SOL
        let postTokenBal = 0
        try {
          const bal = await connection.getTokenAccountBalance(treasuryAta)
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
            'buildHarvestFeesTransaction',
            `${harvestResult.message} — harvested ${(tokensHarvested / 1e6).toFixed(2)} tokens sig=${harvestSig.slice(0, 8)}...`,
          )
        } else {
          ok(
            'buildHarvestFeesTransaction',
            `${harvestResult.message} — tx succeeded (no withheld fees) sig=${harvestSig.slice(0, 8)}...`,
          )
        }
      } catch (e: any) {
        fail('buildHarvestFeesTransaction', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // 19. Vault-Routed Liquidation (was 20, buyback section removed in V33)
      // ------------------------------------------------------------------
      log('\n[20] Vault-Routed Liquidation (borrow → time travel → liquidate via vault)')

      // Deposit more SOL for liquidation payment
      try {
        const depositResult = await buildDepositVaultTransaction(connection, {
          depositor: walletAddr,
          vault_creator: walletAddr,
          amount_sol: 10 * LAMPORTS_PER_SOL,
        })
        await signAndSend(connection, wallet, depositResult.transaction)
      } catch (e: any) {
        log(`  Warning: extra deposit failed: ${e.message?.substring(0, 60)}`)
      }

      try {
        // Borrow at ~48% LTV (close to 50% max, easier to push into liquidation)
        const totalTokens3 = await getVaultTokenBalance(connection, mint, wallet.publicKey)
        const collateralAmount = Math.floor(totalTokens3 * 0.5)

        // [V21] Open-long quote (LTV-bound × treasury headroom; no per-user cap).
        const quote = await getBorrowQuote(connection, mint, collateralAmount)
        const collateralValue = quote.collateral_value_sol
        log(
          `  collateral_value: ${(collateralValue / LAMPORTS_PER_SOL).toFixed(4)} SOL, pool_available: ${(quote.pool_available_sol / LAMPORTS_PER_SOL).toFixed(4)}, max borrow: ${(quote.max_borrow_sol / LAMPORTS_PER_SOL).toFixed(4)}`,
        )

        if (quote.max_borrow_sol < 100_000_000) {
          // MIN_BORROW_AMOUNT
          log('  Skipping liquidation test — treasury too small for minimum borrow (0.1 SOL)')
          ok('vault-routed liquidation', 'skipped — treasury lending capacity too low')
        } else {
          log(
            `  Vault tokens: ${(totalTokens3 / 1e6).toFixed(0)}, collateral: ${(collateralAmount / 1e6).toFixed(0)}, value: ${(collateralValue / 1e9).toFixed(4)} SOL, est borrow: ${(quote.max_borrow_sol / 1e9).toFixed(4)} SOL (~${(quote.max_ltv_bps / 100).toFixed(0)}% LTV)`,
          )

          // V21: borrow size is auto-clamped; post token collateral via vault.
          const openResult = await buildOpenLongTransaction(connection, {
            mint,
            borrower: walletAddr,
            collateral: collateralAmount,
            vault: walletAddr,
          })
          await signAndSend(connection, wallet, openResult.transaction)
          ok('open long for liquidation (vault)', openResult.message)

          // Time travel to push the long's HEALTH LTV (debt / vault-tokens value =
          // collateral + bought) past the 65% threshold via interest accrual. This
          // position opens at ~30% health LTV, so it needs ~2.1x debt growth — and at
          // the [V21] 1.5%/epoch rate (was 2%) that's ~75 epochs. Using 100 for margin.
          const FULL_EPOCH_SLOTS = 1_512_000 // ~7 days
          const slotsToTravel = FULL_EPOCH_SLOTS * 100
          log(`  Time traveling ${slotsToTravel} slots (~700 days)...`)
          const currentSlot = await connection.getSlot()
          await fetch('http://127.0.0.1:8899', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              jsonrpc: '2.0',
              id: 1,
              method: 'surfnet_timeTravel',
              params: [{ absoluteSlot: currentSlot + slotsToTravel }],
            }),
          })
          await new Promise((r) => setTimeout(r, 500))
          ok('time travel', `+${slotsToTravel} slots`)

          // Regression guard: verify the SDK's off-chain interest projection flips
          // health to 'liquidatable' after time-travel, without any on-chain
          // instruction touching the position. If this fails, getPosition has
          // stopped projecting accrued_interest to the current slot.
          const postTravelLoan = await getPosition(connection, mint, vaultOwner, 'long', 0)
          log(
            `  Post-travel (projected): health=${postTravelLoan.health}, LTV=${postTravelLoan.current_ltv_bps != null ? (postTravelLoan.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}, interest=${(postTravelLoan.accrued_interest / LAMPORTS_PER_SOL).toFixed(4)} SOL (stored=${(postTravelLoan.accrued_interest_stored / LAMPORTS_PER_SOL).toFixed(4)})`,
          )
          if (postTravelLoan.health !== 'liquidatable') {
            throw new Error(
              `SDK projection regression: long should be 'liquidatable' post time-travel, got '${postTravelLoan.health}'`,
            )
          }
          ok(
            'projected health (pre-liquidation)',
            `liquidatable off-chain without on-chain accrual touch`,
          )

          // Liquidate via vault — a different linked wallet acts as liquidator.
          // A long liquidation has the liquidator PAY the position's SOL debt
          // (it receives the seized tokens + bonus), so fund it with the full
          // owed amount + gas, not just gas.
          const liquidator = Keypair.generate()
          const liqFunding = postTravelLoan.total_owed + Math.floor(0.2 * LAMPORTS_PER_SOL)
          const fundLiqTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: liquidator.publicKey,
              lamports: liqFunding,
            }),
          )
          const { blockhash: liqBh } = await connection.getLatestBlockhash()
          fundLiqTx.recentBlockhash = liqBh
          fundLiqTx.feePayer = wallet.publicKey
          await signAndSend(connection, wallet, fundLiqTx)

          // Link liquidator to vault
          const linkLiqResult = await buildLinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_link: liquidator.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, linkLiqResult.transaction)

          const vaultBefore = await getVault(connection, walletAddr)
          const liqResult = await buildLiquidateLongTransaction(connection, {
            mint,
            liquidator: liquidator.publicKey.toBase58(),
            borrower: walletAddr,
            vault: walletAddr,
          })

          const liqSig = await signAndSend(connection, liquidator, liqResult.transaction)
          const vaultAfter = await getVault(connection, walletAddr)
          ok(
            'buildLiquidateLongTransaction (vault)',
            `vault_sol_delta=${((vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)).toFixed(4)} SOL sig=${liqSig.slice(0, 8)}...`,
          )

          // Verify position state after liquidation
          try {
            const posAfterLiq = await getPosition(connection, mint, vaultOwner, 'long', 0)
            log(
              `  After liquidation: debt=${(posAfterLiq.debt_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, collateral=${(posAfterLiq.collateral_amount / 1e6).toFixed(0)} tokens, health=${posAfterLiq.health}`,
            )
            ok(
              'getPosition long (after liquidation)',
              `health=${posAfterLiq.health}, remaining_debt=${(posAfterLiq.debt_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL`,
            )
          } catch (e: any) {
            fail('getPosition long (after liquidation)', e)
          }

          // Unlink liquidator
          const unlinkLiqResult = await buildUnlinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_unlink: liquidator.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, unlinkLiqResult.transaction)
        } // end else (achievable LTV high enough for liquidation)
      } catch (e: any) {
        fail('vault-routed liquidation', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // M7. Short Liquidation (open short → time travel → liquidate_short)
      // ------------------------------------------------------------------
      log('\n[M7] Short Liquidation (open → time travel → liquidate_short via vault)')

      // Deposit SOL for short collateral
      try {
        const depositResult = await buildDepositVaultTransaction(connection, {
          depositor: walletAddr,
          vault_creator: walletAddr,
          amount_sol: 5 * LAMPORTS_PER_SOL,
        })
        await signAndSend(connection, wallet, depositResult.transaction)
      } catch (e: any) {
        log(`  Warning: deposit for short liq failed: ${e.message?.substring(0, 60)}`)
      }

      try {
        // Open a short at ~48% LTV — compute tokens to borrow based on pool price
        const shortCollateral = Math.floor(2 * LAMPORTS_PER_SOL)

        const vaultBefore = await getVault(connection, walletAddr)
        if ((vaultBefore?.sol_balance || 0) < 2) {
          log('  Skipping short liquidation — vault SOL too low')
          ok('short liquidation', 'skipped — insufficient vault SOL')
        } else {
          // [V21] open_short auto-clamps borrowed tokens to collateral × LTV
          // (no tokens_to_borrow knob), so we just post SOL collateral.
          log(
            `  Opening short against ${(shortCollateral / LAMPORTS_PER_SOL).toFixed(1)} SOL collateral (borrow auto-clamped)`,
          )

          const openResult = await buildOpenShortTransaction(connection, {
            mint,
            shorter: walletAddr,
            collateral: shortCollateral,
            vault: walletAddr,
          })
          await signAndSend(connection, wallet, openResult.transaction)
          ok('open short for liquidation', openResult.message)

          // Verify position is healthy before time travel
          const posBefore = await getPosition(connection, mint, vaultOwner, 'short', 0)
          log(
            `  Pre-liquidation: debt=${(posBefore.debt_amount / 1e6).toFixed(0)} tokens, LTV=${posBefore.current_ltv_bps !== null ? (posBefore.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}, health=${posBefore.health}`,
          )

          // Time travel ~280 days to accrue enough interest to push LTV past 65% threshold
          // [V21] LTV is sized against the live position_sol_vault (posted
          // collateral + token-sale proceeds ≈ 1.4× the posted SOL), so a short
          // needs ~2.1× debt growth to breach 65% — ~80 epochs of interest.
          const FULL_EPOCH_SLOTS2 = 1_512_000 // ~7 days
          const slotsToTravel2 = FULL_EPOCH_SLOTS2 * 80
          log(`  Time traveling ${slotsToTravel2} slots (~560 days)...`)
          const currentSlot2 = await connection.getSlot()
          await fetch('http://127.0.0.1:8899', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              jsonrpc: '2.0',
              id: 1,
              method: 'surfnet_timeTravel',
              params: [{ absoluteSlot: currentSlot2 + slotsToTravel2 }],
            }),
          })
          await new Promise((r) => setTimeout(r, 500))
          ok('time travel (short liq)', `+${slotsToTravel2} slots`)

          // Regression guard: same projection check for shorts. Interest accrues
          // in tokens, and debt_value_sol should grow relative to sol_collateral.
          const postTravelShort = await getPosition(connection, mint, vaultOwner, 'short', 0)
          log(
            `  Post-travel short (projected): health=${postTravelShort.health}, LTV=${postTravelShort.current_ltv_bps != null ? (postTravelShort.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}, interest_tokens=${(postTravelShort.accrued_interest / 1e6).toFixed(0)} (stored=${(postTravelShort.accrued_interest_stored / 1e6).toFixed(0)})`,
          )
          if (postTravelShort.health !== 'liquidatable') {
            throw new Error(
              `SDK projection regression: short should be 'liquidatable' post time-travel, got '${postTravelShort.health}'`,
            )
          }
          ok(
            'projected short health (pre-liquidation)',
            `liquidatable off-chain without on-chain accrual touch`,
          )

          // Create a liquidator and link to vault. [V21] The SDK now auto-acquires
          // the cover tokens via a prepended DeepPool buy, so the liquidator must
          // FRONT that buy in SOL (~the covered debt value); the seize (debt +
          // bonus) repays it within the same tx. Fund enough to front the buy.
          const shortLiquidator = Keypair.generate()
          const fundShortLiqTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: shortLiquidator.publicKey,
              lamports: 3 * LAMPORTS_PER_SOL,
            }),
          )
          const { blockhash: shortLiqBh } = await connection.getLatestBlockhash()
          fundShortLiqTx.recentBlockhash = shortLiqBh
          fundShortLiqTx.feePayer = wallet.publicKey
          await signAndSend(connection, wallet, fundShortLiqTx)

          const linkShortLiq = await buildLinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_link: shortLiquidator.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, linkShortLiq.transaction)

          // Liquidate short
          const vaultBeforeLiq = await getVault(connection, walletAddr)
          const liqShortResult = await buildLiquidateShortTransaction(connection, {
            mint,
            liquidator: shortLiquidator.publicKey.toBase58(),
            borrower: walletAddr,
            vault: walletAddr,
          })
          const liqShortSig = await signAndSend(
            connection,
            shortLiquidator,
            liqShortResult.transaction,
          )
          const vaultAfterLiq = await getVault(connection, walletAddr)
          ok(
            'buildLiquidateShortTransaction (vault)',
            `vault_sol_delta=${((vaultAfterLiq?.sol_balance || 0) - (vaultBeforeLiq?.sol_balance || 0)).toFixed(4)} SOL sig=${liqShortSig.slice(0, 8)}...`,
          )

          // Verify position state after short liquidation
          try {
            const posAfterShortLiq = await getPosition(connection, mint, vaultOwner, 'short', 0)
            log(
              `  After short liquidation: debt=${(posAfterShortLiq.debt_amount / 1e6).toFixed(0)} tokens, collateral=${(posAfterShortLiq.collateral_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, health=${posAfterShortLiq.health}`,
            )
            ok(
              'getPosition short (after liquidation)',
              `health=${posAfterShortLiq.health}, remaining_debt=${(posAfterShortLiq.debt_amount / 1e6).toFixed(0)} tokens`,
            )
          } catch (e: any) {
            fail('getPosition short (after liquidation)', e)
          }

          // Unlink liquidator
          const unlinkShortLiq = await buildUnlinkWalletTransaction(connection, {
            authority: walletAddr,
            vault_creator: walletAddr,
            wallet_to_unlink: shortLiquidator.publicKey.toBase58(),
          })
          await signAndSend(connection, wallet, unlinkShortLiq.transaction)
        } // end else (vault SOL sufficient)
      } catch (e: any) {
        fail('short liquidation', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }

      // ------------------------------------------------------------------
      // 22. Vault-Routed Claim Protocol Rewards
      // ------------------------------------------------------------------
      log('\n[22] Vault-Routed Claim Protocol Rewards')
      try {
        const [protocolTreasuryPda] = getProtocolTreasuryPda()

        const SLOTS_8_DAYS = Math.floor((8 * 24 * 60 * 60 * 1000) / 400)

        // Fund protocol treasury so rewards are distributable
        const airdropSig = await connection.requestAirdrop(
          protocolTreasuryPda,
          1500 * LAMPORTS_PER_SOL,
        )
        await connection.confirmTransaction(airdropSig)

        // Step 1: Time travel + advance protocol epoch (moves trades to "previous")
        let slot = await connection.getSlot()
        await fetch('http://127.0.0.1:8899', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'surfnet_timeTravel',
            params: [{ absoluteSlot: slot + SLOTS_8_DAYS }],
          }),
        })
        await new Promise((r) => setTimeout(r, 500))

        const primeEpoch = await buildAdvanceProtocolEpochTransaction(connection, {
          payer: walletAddr,
        })
        await signAndSend(connection, wallet, primeEpoch.transaction)
        ok('advance protocol epoch (prime)', 'epoch advanced')

        // Step 2: Generate >= 10 SOL volume via bonding curve buys
        // V27: 3 SOL on a fresh token would yield ~30M tokens (over 20M wallet cap).
        // Use 0.5 SOL per buy across 20 tokens (10 SOL total) to stay under cap.
        const volNames = Array.from({ length: 20 }, (_, i) => `Vol ${String.fromCharCode(65 + i)}`)
        for (const vname of volNames) {
          const volToken = await buildCreateTokenTransaction(connection, {
            creator: walletAddr,
            name: vname,
            symbol: vname.replace(' ', ''),
            metadata_uri: 'https://example.com/vol.json',
          })
          await signAndSend(connection, wallet, volToken.transaction, true)

          const volBuy = await buildDirectBuyTransaction(connection, {
            mint: volToken.mint.toBase58(),
            buyer: walletAddr,
            amount_sol: Math.floor(0.5 * LAMPORTS_PER_SOL),
            slippage_bps: 1000,
          })
          await signAndSend(connection, wallet, volBuy.transaction, true)
        }
        ok('volume buys', '10 SOL across 20 tokens for epoch eligibility')

        // Step 3: Time travel 8 days + advance again
        slot = await connection.getSlot()
        log(`  Time traveling ${SLOTS_8_DAYS} slots (~8 days) for next epoch...`)
        await fetch('http://127.0.0.1:8899', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'surfnet_timeTravel',
            params: [{ absoluteSlot: slot + SLOTS_8_DAYS }],
          }),
        })
        await new Promise((r) => setTimeout(r, 500))

        const advanceEpoch = await buildAdvanceProtocolEpochTransaction(connection, {
          payer: walletAddr,
        })
        await signAndSend(connection, wallet, advanceEpoch.transaction)
        ok('advance protocol epoch', 'epoch advanced for claim')

        // Verify getProtocolTreasuryState + getUserStats show the rolled-over epoch
        // with non-zero previous-epoch volume and a distributable amount available.
        const treasuryPre = await getProtocolTreasuryState(connection)
        if (!treasuryPre) throw new Error('ProtocolTreasury not found after epoch advance')
        log(
          `  ProtocolTreasury: epoch=${treasuryPre.current_epoch}, prev_epoch_volume=${treasuryPre.total_volume_previous_epoch_sol.toFixed(2)} SOL, distributable=${treasuryPre.distributable_amount_sol.toFixed(4)} SOL, balance=${treasuryPre.current_balance_sol.toFixed(2)} SOL`,
        )
        if (treasuryPre.total_volume_previous_epoch_sol <= 0) {
          throw new Error(
            `expected previous-epoch volume > 0 after epoch roll, got ${treasuryPre.total_volume_previous_epoch_sol}`,
          )
        }
        ok(
          'getProtocolTreasuryState (pre-claim)',
          `epoch=${treasuryPre.current_epoch}, distributable=${treasuryPre.distributable_amount_sol.toFixed(4)} SOL`,
        )

        const statsPre = await getUserStats(connection, walletAddr)
        if (!statsPre) throw new Error('UserStats not found after volume activity')
        log(
          `  UserStats: total_volume=${statsPre.total_volume_sol.toFixed(2)} SOL, prev_epoch=${statsPre.volume_previous_epoch_sol.toFixed(2)} SOL, claimed=${statsPre.total_rewards_claimed_sol.toFixed(4)} SOL, last_claimed_epoch=${statsPre.last_epoch_claimed}`,
        )
        if (statsPre.volume_previous_epoch_sol <= 0) {
          throw new Error(
            `expected user's previous-epoch volume > 0, got ${statsPre.volume_previous_epoch_sol}`,
          )
        }
        ok(
          'getUserStats (pre-claim)',
          `prev_epoch_volume=${statsPre.volume_previous_epoch_sol.toFixed(2)} SOL`,
        )

        // Claim protocol rewards via vault
        const vaultBefore = await getVault(connection, walletAddr)
        const claimResult = await buildClaimProtocolRewardsTransaction(connection, {
          user: walletAddr,
          vault: walletAddr,
        })
        const claimSig = await signAndSend(connection, wallet, claimResult.transaction)
        const vaultAfter = await getVault(connection, walletAddr)
        const received = (vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)
        ok(
          'buildClaimProtocolRewardsTransaction (vault)',
          `vault_received=${received.toFixed(6)} SOL sig=${claimSig.slice(0, 8)}...`,
        )

        // Post-claim: UserStats.total_rewards_claimed should increase by ~received,
        // last_epoch_claimed should equal current_epoch, and ProtocolTreasury.total_distributed
        // should have incremented. Regression guard for the two new readers.
        const statsPost = await getUserStats(connection, walletAddr)
        if (!statsPost) throw new Error('UserStats disappeared after claim')
        const rewardsDelta =
          statsPost.total_rewards_claimed_sol - statsPre.total_rewards_claimed_sol
        log(
          `  UserStats (post-claim): claimed=${statsPost.total_rewards_claimed_sol.toFixed(6)} SOL (+${rewardsDelta.toFixed(6)}), last_claimed_epoch=${statsPost.last_epoch_claimed}`,
        )
        if (rewardsDelta <= 0) {
          throw new Error(`expected rewards_claimed to increase after claim, delta=${rewardsDelta}`)
        }
        // last_epoch_claimed stores the epoch whose rewards were just claimed —
        // which is the previous epoch (current_epoch - 1), since rewards only
        // become claimable after the epoch that generated them has rolled over.
        const expectedClaimedEpoch = treasuryPre.current_epoch - 1
        if (statsPost.last_epoch_claimed !== expectedClaimedEpoch) {
          throw new Error(
            `expected last_epoch_claimed=${expectedClaimedEpoch} (current_epoch-1) after claim, got ${statsPost.last_epoch_claimed}`,
          )
        }
        ok(
          'getUserStats (post-claim)',
          `rewards_claimed +${rewardsDelta.toFixed(6)} SOL, last_claimed_epoch=${statsPost.last_epoch_claimed}`,
        )

        const treasuryPost = await getProtocolTreasuryState(connection)
        if (!treasuryPost) throw new Error('ProtocolTreasury disappeared after claim')
        const distributedDelta =
          treasuryPost.total_distributed_sol - treasuryPre.total_distributed_sol
        log(
          `  ProtocolTreasury (post-claim): distributed=${treasuryPost.total_distributed_sol.toFixed(6)} SOL (+${distributedDelta.toFixed(6)}), balance=${treasuryPost.current_balance_sol.toFixed(2)} SOL`,
        )
        if (distributedDelta <= 0) {
          throw new Error(
            `expected protocol total_distributed to increase after claim, delta=${distributedDelta}`,
          )
        }
        ok(
          'getProtocolTreasuryState (post-claim)',
          `total_distributed +${distributedDelta.toFixed(6)} SOL`,
        )
      } catch (e: any) {
        fail('vault-routed claim protocol rewards', e)
        if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
      }
    } catch (e: any) {
      fail('migrate/lending lifecycle', e)
      if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
    }
  }

  // ------------------------------------------------------------------
  // Summary
  // ------------------------------------------------------------------
  console.log('\n' + '='.repeat(60))
  console.log(`RESULTS: ${passed} passed, ${failed} failed`)
  console.log('='.repeat(60))

  if (failed > 0) process.exit(1)
}

main().catch((err) => {
  console.error('\nFATAL:', err)
  process.exit(1)
})
