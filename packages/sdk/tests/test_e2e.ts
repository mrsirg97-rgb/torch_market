/**
 * SDK E2E Test against Surfpool (mainnet fork)
 *
 * Tests: create token → vault lifecycle → buy (direct + vault) → sell → star → messages
 * Then: bond to completion → migrate → margin stress tests (lending + shorts)
 * Margin: getLendingInfo → stress borrow (near-max LTV) → partial repay → full repay → position verification
 *         → short open → getShortPosition → partial close → full close
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
  buildStarTransaction,
  buildMigrateTransaction,
  buildBorrowTransaction,
  buildRepayTransaction,
  buildLiquidateTransaction,
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
  getLoanPosition,
  getShortPosition,
  getTorchVaultPda,
  getBondingCurvePda,
  getProtocolTreasuryPda,
  getTokenTreasuryPda,
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
      `  │    Treasury tokens: ${(buyQuote.tokens_to_treasury / 1e6).toFixed(2).padStart(15)}         │`,
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
  // 12. Star Token (via vault — can't star your own, so link starrer to vault)
  // ------------------------------------------------------------------
  log('\n[12] Star Token (via vault)')
  const starrer = Keypair.generate()
  try {
    // Fund starrer with gas only
    const fundTx = new Transaction().add(
      SystemProgram.transfer({
        fromPubkey: wallet.publicKey,
        toPubkey: starrer.publicKey,
        lamports: 0.02 * LAMPORTS_PER_SOL,
      }),
    )
    const { blockhash } = await connection.getLatestBlockhash()
    fundTx.recentBlockhash = blockhash
    fundTx.feePayer = wallet.publicKey
    await signAndSend(connection, wallet, fundTx, true)

    // Link starrer to vault so vault pays the 0.05 SOL
    const linkResult = await buildLinkWalletTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      wallet_to_link: starrer.publicKey.toBase58(),
    })
    await signAndSend(connection, wallet, linkResult.transaction)

    const result = await buildStarTransaction(connection, {
      mint,
      user: starrer.publicKey.toBase58(),
      vault: walletAddr,
    })
    const sig = await signAndSend(connection, starrer, result.transaction)
    ok('buildStarTransaction (vault)', `sig=${sig.slice(0, 8)}...`)

    // Unlink starrer
    const unlinkResult = await buildUnlinkWalletTransaction(connection, {
      authority: walletAddr,
      vault_creator: walletAddr,
      wallet_to_unlink: starrer.publicKey.toBase58(),
    })
    await signAndSend(connection, wallet, unlinkResult.transaction)
  } catch (e: any) {
    fail('buildStarTransaction (vault)', e)
  }

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
        const treasurySol = Number(tr.sol_balance.toString()) / LAMPORTS_PER_SOL
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
        log(`  │  Vote Vault:       ${voteVault.toFixed(0).padStart(15)} tokens  │`)
        log(`  │  Pool Tokens:      ${poolTokens.toFixed(0).padStart(15)} tokens  │`)
        log(`  │  Excess Burned:    ${excessBurned.toFixed(0).padStart(15)} tokens  │`)
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
        log(
          `  │  Excess Burn %:    ${((excessBurned / CURVE_SUPPLY) * 100).toFixed(1).padStart(14)}%         │`,
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
      // M1. getLendingInfo — verify pool params after migration
      // ------------------------------------------------------------------
      log('\n[M1] getLendingInfo — pool parameters')
      try {
        const info = await getLendingInfo(connection, mint)
        log(
          `  interest_rate=${info.interest_rate_bps}bps, max_ltv=${info.max_ltv_bps}bps, liq_threshold=${info.liquidation_threshold_bps}bps`,
        )
        log(
          `  utilization_cap=${info.utilization_cap_bps}bps, borrow_multiplier=${info.borrow_share_multiplier}x`,
        )
        log(
          `  treasury_sol_available=${(info.treasury_sol_available / LAMPORTS_PER_SOL).toFixed(4)} SOL, active_loans=${info.active_loans}`,
        )
        if (
          info.interest_rate_bps === 200 &&
          info.max_ltv_bps > 0 &&
          info.max_ltv_bps <= 5000 &&
          info.liquidation_threshold_bps === 6500
        ) {
          ok(
            'getLendingInfo',
            `params correct, treasury_available=${(info.treasury_sol_available / LAMPORTS_PER_SOL).toFixed(4)} SOL`,
          )
        } else {
          fail('getLendingInfo', { message: 'unexpected lending params' })
        }
      } catch (e: any) {
        fail('getLendingInfo', e)
      }

      // ------------------------------------------------------------------
      // M1b. getTreasuryState — verify per-token Treasury reader
      // ------------------------------------------------------------------
      log('\n[M1b] getTreasuryState — per-token treasury reader')
      try {
        const ts = await getTreasuryState(connection, mint)
        if (!ts) throw new Error('Treasury not found for migrated token')
        log(
          `  address=${ts.address.slice(0, 16)}...  sol_balance=${ts.sol_balance_sol.toFixed(4)} SOL  tokens_held=${(ts.tokens_held / 1e6).toFixed(0)}  stars=${ts.total_stars}`,
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
        if (ts.sol_balance_sol <= 0) {
          throw new Error(
            `expected Treasury sol_balance > 0 after bonding, got ${ts.sol_balance_sol}`,
          )
        }
        ok(
          'getTreasuryState',
          `sol=${ts.sol_balance_sol.toFixed(4)} SOL, baseline_initialized=${ts.baseline_initialized}`,
        )
      } catch (e: any) {
        fail('getTreasuryState', e)
      }

      // ------------------------------------------------------------------
      // M2. Stress borrow — near-max LTV, verify position state
      // ------------------------------------------------------------------
      log('\n[M2] Stress Borrow — near-max LTV with position verification')
      let stressBorrowActive = false // track whether we have an active loan for later tests

      try {
        const totalTokens = await getVaultTokenBalance(connection, mint, wallet.publicKey)
        log(`  Vault token balance: ${(totalTokens / 1e6).toFixed(0)} tokens`)

        // Use 40% as collateral (save rest for short tests)
        const collateralAmount = Math.floor(totalTokens * 0.4)
        const quote = await getBorrowQuote(connection, mint, collateralAmount)
        log(
          `  Pool available: ${(quote.pool_available_sol / LAMPORTS_PER_SOL).toFixed(4)}, per-user cap: ${(quote.per_user_cap_sol / LAMPORTS_PER_SOL).toFixed(4)}, max borrow: ${(quote.max_borrow_sol / LAMPORTS_PER_SOL).toFixed(4)}`,
        )

        // Borrow at ~45% LTV (close to 50% max, stress test)
        const targetBorrow = Math.floor(quote.collateral_value_sol * 0.45)
        const borrowAmount = Math.min(targetBorrow, quote.max_borrow_sol)

        if (borrowAmount < 100_000_000) {
          // MIN_BORROW_AMOUNT
          log('  Skipping — lending capacity too low for stress test')
          ok('stress borrow', 'skipped — lending capacity too low')
        } else {
          const targetLtv =
            quote.collateral_value_sol > 0
              ? Math.floor((borrowAmount / quote.collateral_value_sol) * 10000)
              : 0
          log(
            `  Collateral: ${(collateralAmount / 1e6).toFixed(0)} tokens (value: ${(quote.collateral_value_sol / LAMPORTS_PER_SOL).toFixed(4)} SOL), borrowing: ${(borrowAmount / LAMPORTS_PER_SOL).toFixed(4)} SOL (~${(targetLtv / 100).toFixed(0)}% LTV)`,
          )

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
            'stress borrow',
            `${(borrowAmount / LAMPORTS_PER_SOL).toFixed(4)} SOL at ~${(targetLtv / 100).toFixed(0)}% LTV, vault_received=${solReceived.toFixed(4)} SOL sig=${borrowSig.slice(0, 8)}...`,
          )
          stressBorrowActive = true

          // ------------------------------------------------------------------
          // M3. getLoanPosition — verify position state after borrow
          // ------------------------------------------------------------------
          log('\n[M3] getLoanPosition — verify active position')
          try {
            const pos = await getLoanPosition(connection, mint, walletAddr)
            log(
              `  collateral=${(pos.collateral_amount / 1e6).toFixed(0)} tokens, borrowed=${(pos.borrowed_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, interest=${pos.accrued_interest}, health=${pos.health}`,
            )
            log(
              `  collateral_value=${pos.collateral_value_sol !== null ? (pos.collateral_value_sol / LAMPORTS_PER_SOL).toFixed(4) + ' SOL' : 'null'}, LTV=${pos.current_ltv_bps !== null ? (pos.current_ltv_bps / 100).toFixed(1) + '%' : 'null'}`,
            )

            if (pos.health === 'healthy' && pos.borrowed_amount > 0 && pos.collateral_amount > 0) {
              ok(
                'getLoanPosition (active)',
                `health=${pos.health}, LTV=${pos.current_ltv_bps !== null ? (pos.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}`,
              )
            } else {
              fail('getLoanPosition (active)', {
                message: `unexpected: health=${pos.health} borrowed=${pos.borrowed_amount}`,
              })
            }
          } catch (e: any) {
            fail('getLoanPosition (active)', e)
          }

          // ------------------------------------------------------------------
          // M4. Partial repay — repay half, verify position still active
          // ------------------------------------------------------------------
          log('\n[M4] Partial Repay — half of debt')
          try {
            const halfDebt = Math.floor(borrowAmount / 2)
            log(
              `  Repaying ${(halfDebt / LAMPORTS_PER_SOL).toFixed(4)} SOL (half of ${(borrowAmount / LAMPORTS_PER_SOL).toFixed(4)})`,
            )

            const repayResult = await buildRepayTransaction(connection, {
              mint,
              borrower: walletAddr,
              sol_amount: halfDebt,
              vault: walletAddr,
            })
            const repaySig = await signAndSend(connection, wallet, repayResult.transaction)
            ok('partial repay', `${repayResult.message} sig=${repaySig.slice(0, 8)}...`)

            // Verify position still active with reduced debt
            const posAfter = await getLoanPosition(connection, mint, walletAddr)
            log(
              `  After partial repay: borrowed=${(posAfter.borrowed_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, health=${posAfter.health}`,
            )
            if (posAfter.borrowed_amount > 0 && posAfter.borrowed_amount < borrowAmount) {
              ok('getLoanPosition (after partial repay)', `debt reduced, health=${posAfter.health}`)
            } else {
              fail('getLoanPosition (after partial repay)', {
                message: `unexpected borrowed_amount=${posAfter.borrowed_amount}`,
              })
            }
          } catch (e: any) {
            fail('partial repay', e)
          }

          // ------------------------------------------------------------------
          // M5. Full repay — close the position
          // ------------------------------------------------------------------
          log('\n[M5] Full Repay — close position')
          try {
            const repayResult = await buildRepayTransaction(connection, {
              mint,
              borrower: walletAddr,
              sol_amount: borrowAmount, // overpay to ensure full close
              vault: walletAddr,
            })
            const repaySig = await signAndSend(connection, wallet, repayResult.transaction)
            ok('full repay', `${repayResult.message} sig=${repaySig.slice(0, 8)}...`)
            stressBorrowActive = false

            // Verify position closed
            const posAfter = await getLoanPosition(connection, mint, walletAddr)
            if (posAfter.health === 'none' || posAfter.borrowed_amount === 0) {
              ok('getLoanPosition (after full repay)', 'position closed')
            } else {
              fail('getLoanPosition (after full repay)', {
                message: `still active: borrowed=${posAfter.borrowed_amount}`,
              })
            }
          } catch (e: any) {
            fail('full repay', e)
          }
        } // end else (borrowAmount >= MIN_BORROW)
      } catch (e: any) {
        fail('stress borrow', e)
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

        // Post 1 SOL as collateral, borrow 5000 tokens (5x minimum)
        const shortCollateral = Math.floor(1 * LAMPORTS_PER_SOL)
        const tokensToBorrow = 5_000_000_000 // 5,000 tokens (6 decimals)

        if (vaultSolInSol < 1.0) {
          log('  Skipping short — vault SOL too low for 1 SOL collateral')
          ok('short selling stress', 'skipped — insufficient vault SOL')
        } else {
          // Open short
          const openResult = await buildOpenShortTransaction(connection, {
            mint,
            shorter: walletAddr,
            sol_collateral: shortCollateral,
            tokens_to_borrow: tokensToBorrow,
            vault: walletAddr,
          })
          const openSig = await signAndSend(connection, wallet, openResult.transaction)
          const vaultAfterOpen = await getVault(connection, walletAddr)
          const solSpent = (vaultBeforeShort?.sol_balance || 0) - (vaultAfterOpen?.sol_balance || 0)
          ok(
            'open short',
            `${openResult.message} collateral=${(solSpent / LAMPORTS_PER_SOL).toFixed(4)} SOL sig=${openSig.slice(0, 8)}...`,
          )

          // Verify short position via getShortPosition
          log('\n  Verifying short position...')
          try {
            const shortPos = await getShortPosition(connection, mint, walletAddr)
            log(
              `  sol_collateral=${(shortPos.sol_collateral / LAMPORTS_PER_SOL).toFixed(4)} SOL, tokens_borrowed=${(shortPos.tokens_borrowed / 1e6).toFixed(0)}, interest=${(shortPos.accrued_interest / 1e6).toFixed(0)}`,
            )
            log(
              `  debt_value=${shortPos.debt_value_sol !== null ? (shortPos.debt_value_sol / LAMPORTS_PER_SOL).toFixed(4) + ' SOL' : 'null'}, LTV=${shortPos.current_ltv_bps !== null ? (shortPos.current_ltv_bps / 100).toFixed(1) + '%' : 'null'}, health=${shortPos.health}`,
            )

            if (
              shortPos.health === 'healthy' &&
              shortPos.tokens_borrowed > 0 &&
              shortPos.sol_collateral > 0
            ) {
              ok(
                'getShortPosition (active)',
                `health=${shortPos.health}, LTV=${shortPos.current_ltv_bps !== null ? (shortPos.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}`,
              )
            } else {
              fail('getShortPosition (active)', {
                message: `unexpected: health=${shortPos.health}`,
              })
            }
          } catch (e: any) {
            fail('getShortPosition (active)', e)
          }

          // Partial close — return half the borrowed tokens
          log('\n  Partial close short (50% of tokens)...')
          try {
            const halfTokens = Math.floor(tokensToBorrow / 2)
            const closeResult = await buildCloseShortTransaction(connection, {
              mint,
              shorter: walletAddr,
              token_amount: halfTokens,
              vault: walletAddr,
            })
            const closeSig = await signAndSend(connection, wallet, closeResult.transaction)
            ok(
              'partial close short',
              `returned ${(halfTokens / 1e6).toFixed(0)} tokens sig=${closeSig.slice(0, 8)}...`,
            )

            // Verify position still active with reduced debt
            const posAfter = await getShortPosition(connection, mint, walletAddr)
            log(
              `  After partial close: tokens_borrowed=${(posAfter.tokens_borrowed / 1e6).toFixed(0)}, health=${posAfter.health}`,
            )
            if (posAfter.tokens_borrowed > 0 && posAfter.tokens_borrowed < tokensToBorrow) {
              ok(
                'getShortPosition (after partial close)',
                `debt reduced to ${(posAfter.tokens_borrowed / 1e6).toFixed(0)} tokens`,
              )
            } else {
              fail('getShortPosition (after partial close)', {
                message: `unexpected: tokens_borrowed=${posAfter.tokens_borrowed}`,
              })
            }
          } catch (e: any) {
            fail('partial close short', e)
            if (e.logs) console.error('  Logs:', e.logs.slice(-5).join('\n        '))
          }

          // Full close — overpay to cover interest + remaining
          log('\n  Full close short...')
          try {
            const closeResult = await buildCloseShortTransaction(connection, {
              mint,
              shorter: walletAddr,
              token_amount: tokensToBorrow * 2, // overpay to fully close
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
            const posAfter = await getShortPosition(connection, mint, walletAddr)
            if (posAfter.health === 'none' || posAfter.tokens_borrowed === 0) {
              ok('getShortPosition (after full close)', 'position closed')
            } else {
              fail('getShortPosition (after full close)', {
                message: `still active: tokens=${posAfter.tokens_borrowed}`,
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
        const preSolBalance =
          Number(preHarvestData?.treasury?.sol_balance?.toString() || '0') / LAMPORTS_PER_SOL
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
        const postHarvestData = await fetchTokenRaw(connection, new PublicKey(mint))
        const postSolBalance =
          Number(postHarvestData?.treasury?.sol_balance?.toString() || '0') / LAMPORTS_PER_SOL
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

        // Get borrow quote (handles pool price, treasury cap, per-user cap, transfer fee, etc.)
        const quote = await getBorrowQuote(connection, mint, collateralAmount)
        const collateralValue = quote.collateral_value_sol
        let solToBorrow = Math.max(100_000_000, Math.floor(collateralValue * 0.48))
        solToBorrow = Math.min(solToBorrow, quote.max_borrow_sol)
        log(
          `  Pool available: ${(quote.pool_available_sol / LAMPORTS_PER_SOL).toFixed(4)}, per-user cap: ${(quote.per_user_cap_sol / LAMPORTS_PER_SOL).toFixed(4)}, max borrow: ${(quote.max_borrow_sol / LAMPORTS_PER_SOL).toFixed(4)}`,
        )

        // Check if achievable LTV can reach liquidation threshold (65%)
        // With per-user cap, small collateral positions can only borrow a tiny fraction of value
        const achievableLtvBps =
          collateralValue > 0 ? Math.floor((solToBorrow / collateralValue) * 10000) : 0

        if (solToBorrow < 100_000_000) {
          // MIN_BORROW_AMOUNT
          log('  Skipping liquidation test — treasury too small for minimum borrow (0.1 SOL)')
          ok('vault-routed liquidation', 'skipped — treasury lending capacity too low')
        } else if (achievableLtvBps < 3000) {
          // If we can't even reach 30% LTV, interest accrual won't push us to 65% in a reasonable time
          log(
            `  Skipping liquidation test — per-user cap limits LTV to ${(achievableLtvBps / 100).toFixed(1)}% (need ~48% for liquidation test)`,
          )
          ok(
            'vault-routed liquidation',
            `skipped — per-user cap limits achievable LTV to ${(achievableLtvBps / 100).toFixed(1)}%`,
          )
        } else {
          log(
            `  Vault tokens: ${(totalTokens3 / 1e6).toFixed(0)}, collateral: ${(collateralAmount / 1e6).toFixed(0)}, value: ${(collateralValue / 1e9).toFixed(4)} SOL, borrow: ${(solToBorrow / 1e9).toFixed(4)} SOL (~${(achievableLtvBps / 100).toFixed(0)}% LTV)`,
          )

          const borrowResult = await buildBorrowTransaction(connection, {
            mint,
            borrower: walletAddr,
            collateral_amount: collateralAmount,
            sol_to_borrow: solToBorrow,
            vault: walletAddr,
          })
          await signAndSend(connection, wallet, borrowResult.transaction)
          ok('borrow for liquidation (vault)', borrowResult.message)

          // Time travel ~420 days to push LTV past 65% threshold via interest accrual
          // At 23x borrow multiplier, effective LTV is ~31%. Need ~55 epochs (385 days)
          // to accrue enough interest at 2%/epoch to breach 65%. Using 60 for margin.
          const FULL_EPOCH_SLOTS = 1_512_000 // ~7 days
          const slotsToTravel = FULL_EPOCH_SLOTS * 60
          log(`  Time traveling ${slotsToTravel} slots (~420 days)...`)
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
          // instruction touching the loan. If this fails, getLoanPosition has
          // stopped projecting accrued_interest to the current slot.
          const postTravelLoan = await getLoanPosition(connection, mint, walletAddr)
          log(
            `  Post-travel (projected): health=${postTravelLoan.health}, LTV=${postTravelLoan.current_ltv_bps != null ? (postTravelLoan.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}, interest=${(postTravelLoan.accrued_interest / LAMPORTS_PER_SOL).toFixed(4)} SOL (stored=${(postTravelLoan.accrued_interest_stored / LAMPORTS_PER_SOL).toFixed(4)})`,
          )
          if (postTravelLoan.health !== 'liquidatable') {
            throw new Error(
              `SDK projection regression: loan should be 'liquidatable' post time-travel, got '${postTravelLoan.health}'`,
            )
          }
          ok(
            'projected health (pre-liquidation)',
            `liquidatable off-chain without on-chain accrual touch`,
          )

          // Liquidate via vault — a different linked wallet acts as liquidator
          const liquidator = Keypair.generate()
          const fundLiqTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: liquidator.publicKey,
              lamports: 0.05 * LAMPORTS_PER_SOL,
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
          const liqResult = await buildLiquidateTransaction(connection, {
            mint,
            liquidator: liquidator.publicKey.toBase58(),
            borrower: walletAddr,
            vault: walletAddr,
          })

          const liqSig = await signAndSend(connection, liquidator, liqResult.transaction)
          const vaultAfter = await getVault(connection, walletAddr)
          ok(
            'buildLiquidateTransaction (vault)',
            `vault_sol_delta=${((vaultAfter?.sol_balance || 0) - (vaultBefore?.sol_balance || 0)).toFixed(4)} SOL sig=${liqSig.slice(0, 8)}...`,
          )

          // Verify position state after liquidation
          try {
            const posAfterLiq = await getLoanPosition(connection, mint, walletAddr)
            log(
              `  After liquidation: borrowed=${(posAfterLiq.borrowed_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL, collateral=${(posAfterLiq.collateral_amount / 1e6).toFixed(0)} tokens, health=${posAfterLiq.health}`,
            )
            ok(
              'getLoanPosition (after liquidation)',
              `health=${posAfterLiq.health}, remaining_debt=${(posAfterLiq.borrowed_amount / LAMPORTS_PER_SOL).toFixed(4)} SOL`,
            )
          } catch (e: any) {
            fail('getLoanPosition (after liquidation)', e)
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
          // Compute how many tokens to borrow to hit ~48% LTV (DeepPool)
          // LTV = (tokens * pool_sol / pool_tokens) / sol_collateral
          // tokens = LTV * sol_collateral * pool_tokens / pool_sol
          const dp = getDeepPoolAccounts(new PublicKey(mint))
          const [dpAcct, dpTokenBal, dpRent] = await Promise.all([
            connection.getAccountInfo(dp.pool),
            connection.getTokenAccountBalance(dp.tokenVault),
            connection.getMinimumBalanceForRentExemption(129),
          ])
          const poolSol = (dpAcct?.lamports ?? 0) - dpRent
          const poolTokens = Number(dpTokenBal.value.amount)
          // Target 95% of depth-band max LTV for this pool
          const lendingInfo = await getLendingInfo(connection, mint)
          const targetLtv = (lendingInfo.max_ltv_bps * 0.95) / 10000
          let tokensToBorrow = Math.floor((targetLtv * shortCollateral * poolTokens) / poolSol)
          tokensToBorrow = Math.max(tokensToBorrow, 1_000_000_000) // at least MIN_SHORT_TOKENS
          log(
            `  Pool: ${(poolSol / LAMPORTS_PER_SOL).toFixed(2)} SOL / ${(poolTokens / 1e6).toFixed(0)} tokens, borrowing ${(tokensToBorrow / 1e6).toFixed(0)} tokens against ${(shortCollateral / LAMPORTS_PER_SOL).toFixed(1)} SOL collateral`,
          )

          const openResult = await buildOpenShortTransaction(connection, {
            mint,
            shorter: walletAddr,
            sol_collateral: shortCollateral,
            tokens_to_borrow: tokensToBorrow,
            vault: walletAddr,
          })
          await signAndSend(connection, wallet, openResult.transaction)
          ok('open short for liquidation', openResult.message)

          // Verify position is healthy before time travel
          const posBefore = await getShortPosition(connection, mint, walletAddr)
          log(
            `  Pre-liquidation: tokens_borrowed=${(posBefore.tokens_borrowed / 1e6).toFixed(0)}, LTV=${posBefore.current_ltv_bps !== null ? (posBefore.current_ltv_bps / 100).toFixed(1) + '%' : 'n/a'}, health=${posBefore.health}`,
          )

          // Time travel ~280 days to accrue enough interest to push LTV past 65% threshold
          const FULL_EPOCH_SLOTS2 = 1_512_000 // ~7 days
          const slotsToTravel2 = FULL_EPOCH_SLOTS2 * 40
          log(`  Time traveling ${slotsToTravel2} slots (~280 days)...`)
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
          const postTravelShort = await getShortPosition(connection, mint, walletAddr)
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

          // Create a liquidator and link to vault
          const shortLiquidator = Keypair.generate()
          const fundShortLiqTx = new Transaction().add(
            SystemProgram.transfer({
              fromPubkey: wallet.publicKey,
              toPubkey: shortLiquidator.publicKey,
              lamports: 0.05 * LAMPORTS_PER_SOL,
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
            const posAfterShortLiq = await getShortPosition(connection, mint, walletAddr)
            log(
              `  After short liquidation: tokens_borrowed=${(posAfterShortLiq.tokens_borrowed / 1e6).toFixed(0)}, collateral=${(posAfterShortLiq.sol_collateral / LAMPORTS_PER_SOL).toFixed(4)} SOL, health=${posAfterShortLiq.health}`,
            )
            ok(
              'getShortPosition (after liquidation)',
              `health=${posAfterShortLiq.health}, remaining_debt=${(posAfterShortLiq.tokens_borrowed / 1e6).toFixed(0)} tokens`,
            )
          } catch (e: any) {
            fail('getShortPosition (after liquidation)', e)
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
