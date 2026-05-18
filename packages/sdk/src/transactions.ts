/**
 * Transaction builders
 *
 * Build unsigned transactions for buy, sell, create, star, vault, and lending.
 * Agents sign these locally and submit to the network.
 */

import {
  Connection,
  PublicKey,
  Transaction,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  ComputeBudgetProgram,
  Keypair,
} from '@solana/web3.js'
import {
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountIdempotentInstruction,
  TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  unpackAccount,
  getTransferFeeAmount,
} from '@solana/spl-token'
import { BN, Program, AnchorProvider, Wallet } from '@coral-xyz/anchor'
import {
  getBondingCurvePda,
  getTokenTreasuryPda,
  getTreasuryTokenAccount,
  getUserPositionPda,
  getUserStatsPda,
  getGlobalConfigPda,
  getProtocolTreasuryPda,
  getStarRecordPda,
  getLoanPositionPda,
  getCollateralVaultPda,
  getTorchVaultPda,
  getVaultSolPda,
  getVaultWalletLinkPda,
  getTreasuryLockPda,
  getTreasuryLockTokenAccount,
  getShortPositionPda,
  getShortConfigPda,
  getDeepPoolAccounts,
  calculateTokensOut,
  calculateSolOut,
  GlobalConfig,
} from './program'
import { MEMO_PROGRAM_ID, DEEP_POOL_PROGRAM_ID } from './constants'
import { fetchTokenRaw } from './tokens'
import { getBuyQuote, getSellQuote } from './quotes'
import {
  BuyParams,
  DirectBuyParams,
  SellParams,
  BuyQuoteResult,
  CreateTokenParams,
  StarParams,
  MigrateParams,
  BorrowParams,
  RepayParams,
  LiquidateParams,
  OpenShortParams,
  CloseShortParams,
  LiquidateShortParams,
  EnableShortSellingParams,
  ClaimProtocolRewardsParams,
  VaultSwapParams,
  HarvestFeesParams,
  SwapFeesToSolParams,
  AdvanceProtocolEpochParams,
  CreateVaultParams,
  DepositVaultParams,
  WithdrawVaultParams,
  WithdrawTokensParams,
  LinkWalletParams,
  UnlinkWalletParams,
  TransferAuthorityParams,
  ReclaimParams,
  TransactionResult,
  BuyTransactionResult,
  CreateTokenResult,
  WalletAdapter,
} from './types'
import idl from './torch_market.json'

// ============================================================================
// Helpers
// ============================================================================

const MAX_MESSAGE_LENGTH = 500

const makeDummyProvider = (connection: Connection, payer: PublicKey): AnchorProvider => {
  const dummyWallet = {
    publicKey: payer,
    signTransaction: async (t: Transaction) => t,
    signAllTransactions: async (t: Transaction[]) => t,
  }
  return new AnchorProvider(connection, dummyWallet as unknown as Wallet, {})
}

/** Derive vault + wallet link PDAs. Returns nulls if vaultCreatorStr is undefined. */
const deriveVaultAccounts = (
  vaultCreatorStr: string | undefined,
  signer: PublicKey,
): { torchVault: PublicKey | null; walletLink: PublicKey | null } => {
  if (!vaultCreatorStr) return { torchVault: null, walletLink: null }
  const vaultCreator = new PublicKey(vaultCreatorStr)
  const [torchVault] = getTorchVaultPda(vaultCreator)
  const [walletLink] = getVaultWalletLinkPda(signer)
  return { torchVault, walletLink }
}

/** Create vault token ATA instruction (idempotent). */
const createVaultTokenAtaIx = (payer: PublicKey, mint: PublicKey, vaultPda: PublicKey) =>
  createAssociatedTokenAccountIdempotentInstruction(
    payer,
    getAssociatedTokenAddressSync(mint, vaultPda, true, TOKEN_2022_PROGRAM_ID),
    vaultPda,
    mint,
    TOKEN_2022_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID,
  )

/** Get the vault's token ATA address. */
const getVaultTokenAta = (mint: PublicKey, vaultPda: PublicKey) =>
  getAssociatedTokenAddressSync(mint, vaultPda, true, TOKEN_2022_PROGRAM_ID)

/** Add an SPL Memo instruction to a transaction. */
const addMemoIx = (
  tx: Transaction,
  signer: PublicKey,
  message: string | undefined,
  maxLength: number = MAX_MESSAGE_LENGTH,
) => {
  if (!message || message.trim().length === 0) return
  const trimmed = message.trim().slice(0, maxLength)
  if (trimmed.length < message.trim().length && maxLength === MAX_MESSAGE_LENGTH) {
    throw new Error(`Message must be ${MAX_MESSAGE_LENGTH} characters or less`)
  }
  tx.add(
    new TransactionInstruction({
      programId: MEMO_PROGRAM_ID,
      keys: [{ pubkey: signer, isSigner: true, isWritable: false }],
      data: Buffer.from(trimmed, 'utf-8'),
    }),
  )
}

// ── Transaction finalization ────────────────────────────────────────

/**
 * Compile instructions into a VersionedTransaction (v0 message).
 */
const finalizeTransaction = async (
  connection: Connection,
  tx: Transaction,
  feePayer: PublicKey,
): Promise<VersionedTransaction> => {
  const { blockhash } = await connection.getLatestBlockhash()

  const message = new TransactionMessage({
    payerKey: feePayer,
    recentBlockhash: blockhash,
    instructions: tx.instructions,
  }).compileToV0Message()

  return new VersionedTransaction(message)
}

// ============================================================================
// Buy
// ============================================================================

// Internal buy builder shared by both vault and direct variants
const buildBuyTransactionInternal = async (
  connection: Connection,
  mintStr: string,
  buyerStr: string,
  amount_sol: number,
  slippage_bps: number,
  message: string | undefined,
  vaultCreatorStr: string | undefined,
  quote: BuyQuoteResult | undefined,
): Promise<BuyTransactionResult> => {
  const mint = new PublicKey(mintStr)
  const buyer = new PublicKey(buyerStr)

  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) throw new Error(`Token not found: ${mintStr}`)

  const { bondingCurve, treasury } = tokenData

  // Migrated token — route through vault swap on DeepPool
  if (quote?.source === 'dex' || bondingCurve.bonding_complete) {
    if (!vaultCreatorStr) {
      throw new Error(
        'Migrated tokens require vault-based trading. Use buildBuyTransaction with a vault parameter.',
      )
    }
    const resolvedQuote = quote ?? (await getBuyQuote(connection, mintStr, amount_sol))
    const slippage = slippage_bps ?? 100
    const minOut =
      (BigInt(resolvedQuote.min_output_tokens) * BigInt(10000 - slippage)) / BigInt(10000)
    const result = await buildVaultSwapTransaction(connection, {
      mint: mintStr,
      signer: buyerStr,
      vault_creator: vaultCreatorStr,
      amount_in: amount_sol,
      minimum_amount_out: Number(minOut),
      is_buy: true,
      message,
    })
    return {
      ...result,
      message: `Buy ~${resolvedQuote.tokens_to_user / 1e6} tokens for ${amount_sol / 1e9} SOL (via DEX)`,
    }
  }

  // Calculate expected output
  const virtualSol = BigInt(bondingCurve.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bondingCurve.virtual_token_reserves.toString())
  const realSol = BigInt(bondingCurve.real_sol_reserves.toString())
  const bondingTarget = BigInt(bondingCurve.bonding_target.toString())
  const solAmount = BigInt(amount_sol)

  const result = calculateTokensOut(
    solAmount,
    virtualSol,
    virtualTokens,
    realSol,
    100,
    100,
    bondingTarget,
  )

  // [V28] Detect if this buy will complete bonding
  const resolvedTarget = bondingTarget === BigInt(0) ? BigInt('200000000000') : bondingTarget
  const newRealSol = realSol + result.solToCurve
  const willCompleteBonding = newRealSol >= resolvedTarget

  // Apply slippage
  if (slippage_bps < 10 || slippage_bps > 1000) {
    throw new Error(`slippage_bps must be between 10 (0.1%) and 1000 (10%), got ${slippage_bps}`)
  }
  const slippage = slippage_bps
  const minTokens = (result.tokensToUser * BigInt(10000 - slippage)) / BigInt(10000)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [userPositionPda] = getUserPositionPda(bondingCurvePda, buyer)
  const [userStatsPda] = getUserStatsPda(buyer)
  const [globalConfigPda] = getGlobalConfigPda()
  const [protocolTreasuryPda] = getProtocolTreasuryPda()

  const bondingCurveTokenAccount = getAssociatedTokenAddressSync(
    mint,
    bondingCurvePda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  const treasuryTokenAccount = getTreasuryTokenAccount(mint, treasuryPda)
  const buyerTokenAccount = getAssociatedTokenAddressSync(mint, buyer, false, TOKEN_2022_PROGRAM_ID)

  const tx = new Transaction()

  // Create buyer ATA if needed
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      buyer,
      buyerTokenAccount,
      buyer,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  const provider = makeDummyProvider(connection, buyer)
  const program = new Program(idl as unknown, provider)

  // Fetch global config for dev wallet
  const globalConfigAccount = (await (program.account as any).globalConfig.fetch(
    globalConfigPda,
  )) as GlobalConfig

  // Vault accounts (optional — pass null when not using vault)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    buyer,
  )
  let vaultTokenAccount: PublicKey | null = null
  if (torchVaultAccount) {
    vaultTokenAccount = getVaultTokenAta(mint, torchVaultAccount)
    tx.add(createVaultTokenAtaIx(buyer, mint, torchVaultAccount))
  }

  const buyArgs = {
    solAmount: new BN(amount_sol.toString()),
    minTokensOut: new BN(minTokens.toString()),
  }
  const buyBaseAccounts = {
    buyer,
    globalConfig: globalConfigPda,
    devWallet: (globalConfigAccount as any).devWallet || globalConfigAccount.dev_wallet,
    protocolTreasury: protocolTreasuryPda,
    creator: bondingCurve.creator,
    mint,
    bondingCurve: bondingCurvePda,
    tokenVault: bondingCurveTokenAccount,
    tokenTreasury: treasuryPda,
    treasuryTokenAccount,
    buyerTokenAccount,
    userPosition: userPositionPda,
    userStats: userStatsPda,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const buyIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .buyViaVault(buyArgs)
          .accounts({
            ...buyBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods.buy(buyArgs).accounts(buyBaseAccounts).instruction()

  tx.add(buyIx)
  addMemoIx(tx, buyer, message)

  const versionedTx = await finalizeTransaction(connection, tx, buyer)

  // [V28] Build separate migration transaction when this buy completes bonding.
  // Split into two txs because buy + migration exceeds the 1232-byte legacy limit.
  // Program handles treasury reimbursement internally, so this is just a standard migration call.
  let migrationTransaction: VersionedTransaction | undefined
  if (willCompleteBonding) {
    const migResult = await buildMigrateTransaction(connection, {
      mint: mintStr,
      payer: buyerStr,
    })
    migrationTransaction = migResult.transaction
  }

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  const migrateLabel = willCompleteBonding ? ' + migrate to DEX' : ''
  return {
    transaction: versionedTx,
    migrationTransaction,
    message: `Buy ${Number(result.tokensToUser) / 1e6} tokens for ${Number(solAmount) / 1e9} SOL${vaultLabel}${migrateLabel}`,
  }
}

/**
 * Build an unsigned vault-funded buy transaction.
 *
 * The vault pays for the buy. This is the recommended path for AI agents.
 *
 * @param connection - Solana RPC connection
 * @param params - Buy parameters with required vault creator pubkey
 * @returns Unsigned transaction and descriptive message
 */
export const buildBuyTransaction = async (
  connection: Connection,
  params: BuyParams,
): Promise<BuyTransactionResult> => {
  const { mint, buyer, amount_sol, slippage_bps = 100, message, vault, quote } = params
  return buildBuyTransactionInternal(
    connection,
    mint,
    buyer,
    amount_sol,
    slippage_bps,
    message,
    vault,
    quote,
  )
}

/**
 * Build an unsigned direct buy transaction (no vault).
 *
 * The buyer pays from their own wallet. Use this for human-operated wallets only.
 * For AI agents, use buildBuyTransaction with a vault instead.
 *
 * @param connection - Solana RPC connection
 * @param params - Buy parameters (no vault)
 * @returns Unsigned transaction and descriptive message
 */
export const buildDirectBuyTransaction = async (
  connection: Connection,
  params: DirectBuyParams,
): Promise<BuyTransactionResult> => {
  const { mint, buyer, amount_sol, slippage_bps = 100, message, quote } = params
  return buildBuyTransactionInternal(
    connection,
    mint,
    buyer,
    amount_sol,
    slippage_bps,
    message,
    undefined,
    quote,
  )
}

// ── Sign-and-send helpers (Phantom / wallet-integrated flows) ────────

/**
 * Build, simulate, and submit a vault-funded buy via signAndSendTransaction.
 *
 * This is the recommended path for Phantom and other browser wallets.
 * The wallet receives the final, immutable transaction for atomic sign+send,
 * which avoids false-positive "malicious dapp" warnings.
 *
 * @returns Transaction signature on success
 */
export const sendBuy = async (
  connection: Connection,
  wallet: WalletAdapter,
  params: Omit<BuyParams, 'buyer'>,
): Promise<string> => {
  const fullParams: BuyParams = { ...params, buyer: wallet.publicKey.toBase58() }
  const { transaction, migrationTransaction } = await buildBuyTransaction(connection, fullParams)

  const sim = await connection.simulateTransaction(transaction, { sigVerify: false })
  if (sim.value.err) {
    throw new Error(`Buy simulation failed: ${JSON.stringify(sim.value.err)}`)
  }

  const { signature } = await wallet.signAndSendTransaction(transaction)

  if (migrationTransaction) {
    const migSim = await connection.simulateTransaction(migrationTransaction, { sigVerify: false })
    if (!migSim.value.err) {
      await wallet.signAndSendTransaction(migrationTransaction)
    }
  }

  return signature
}

/**
 * Build, simulate, and submit a direct buy (no vault) via signAndSendTransaction.
 *
 * Same Phantom-friendly flow as sendBuy but buyer pays from their own wallet.
 *
 * @returns Transaction signature on success
 */
export const sendDirectBuy = async (
  connection: Connection,
  wallet: WalletAdapter,
  params: Omit<DirectBuyParams, 'buyer'>,
): Promise<string> => {
  const fullParams: DirectBuyParams = { ...params, buyer: wallet.publicKey.toBase58() }
  const { transaction, migrationTransaction } = await buildDirectBuyTransaction(
    connection,
    fullParams,
  )

  const sim = await connection.simulateTransaction(transaction, { sigVerify: false })
  if (sim.value.err) {
    throw new Error(`Buy simulation failed: ${JSON.stringify(sim.value.err)}`)
  }

  const { signature } = await wallet.signAndSendTransaction(transaction)

  if (migrationTransaction) {
    const migSim = await connection.simulateTransaction(migrationTransaction, { sigVerify: false })
    if (!migSim.value.err) {
      await wallet.signAndSendTransaction(migrationTransaction)
    }
  }

  return signature
}

// ============================================================================
// Sell
// ============================================================================

/**
 * Build an unsigned sell transaction.
 *
 * @param connection - Solana RPC connection
 * @param params - Sell parameters (mint, seller, amount_tokens in raw units, optional slippage_bps)
 * @returns Unsigned transaction and descriptive message
 */
export const buildSellTransaction = async (
  connection: Connection,
  params: SellParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    seller: sellerStr,
    amount_tokens,
    slippage_bps = 100,
    message,
    vault: vaultCreatorStr,
    quote,
  } = params

  const mint = new PublicKey(mintStr)
  const seller = new PublicKey(sellerStr)

  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) throw new Error(`Token not found: ${mintStr}`)

  const { bondingCurve } = tokenData

  // Migrated token — route through vault swap on DeepPool
  if (quote?.source === 'dex' || bondingCurve.bonding_complete) {
    if (!vaultCreatorStr) {
      throw new Error(
        'Migrated tokens require vault-based trading. Use buildSellTransaction with a vault parameter.',
      )
    }
    const resolvedQuote = quote ?? (await getSellQuote(connection, mintStr, amount_tokens))
    const slippage = slippage_bps ?? 100
    const minOut = (BigInt(resolvedQuote.min_output_sol) * BigInt(10000 - slippage)) / BigInt(10000)
    const result = await buildVaultSwapTransaction(connection, {
      mint: mintStr,
      signer: sellerStr,
      vault_creator: vaultCreatorStr,
      amount_in: amount_tokens,
      minimum_amount_out: Number(minOut),
      is_buy: false,
      message,
    })
    return {
      ...result,
      message: `Sell ${amount_tokens / 1e6} tokens for ~${resolvedQuote.output_sol / 1e9} SOL (via DEX)`,
    }
  }

  // Calculate expected output
  const virtualSol = BigInt(bondingCurve.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bondingCurve.virtual_token_reserves.toString())
  const tokenAmount = BigInt(amount_tokens)

  const result = calculateSolOut(tokenAmount, virtualSol, virtualTokens)

  // Apply slippage
  if (slippage_bps < 10 || slippage_bps > 1000) {
    throw new Error(`slippage_bps must be between 10 (0.1%) and 1000 (10%), got ${slippage_bps}`)
  }
  const slippage = slippage_bps
  const minSol = (result.solToUser * BigInt(10000 - slippage)) / BigInt(10000)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [userPositionPda] = getUserPositionPda(bondingCurvePda, seller)
  const [userStatsPda] = getUserStatsPda(seller)

  // [V35] Optional accounts — check existence before passing (Anchor needs
  // program ID for None, not a non-existent PDA address)
  const [userPositionInfo, userStatsInfo] = await connection.getMultipleAccountsInfo([
    userPositionPda,
    userStatsPda,
  ])
  const userPositionAccount = userPositionInfo ? userPositionPda : null
  const userStatsAccount = userStatsInfo ? userStatsPda : null

  const bondingCurveTokenAccount = getAssociatedTokenAddressSync(
    mint,
    bondingCurvePda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  const sellerTokenAccount = getAssociatedTokenAddressSync(
    mint,
    seller,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // Vault accounts (optional — pass null when not using vault)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    seller,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(seller, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, seller)
  const program = new Program(idl as unknown, provider)

  const sellArgs = {
    tokenAmount: new BN(amount_tokens.toString()),
    minSolOut: new BN(minSol.toString()),
  }
  // userPosition / userStats are Optional accounts in the Sell context;
  // Anchor accepts null at runtime to skip them but the loosely-typed
  // Program<unknown> typing rejects the union — cast each to PublicKey.
  const sellBaseAccounts = {
    seller,
    mint,
    bondingCurve: bondingCurvePda,
    tokenVault: bondingCurveTokenAccount,
    sellerTokenAccount,
    userPosition: userPositionAccount as PublicKey,
    tokenTreasury: treasuryPda,
    userStats: userStatsAccount as PublicKey,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const sellIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .sellViaVault(sellArgs)
          .accounts({
            ...sellBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods.sell(sellArgs).accounts(sellBaseAccounts).instruction()

  tx.add(sellIx)

  addMemoIx(tx, seller, message)

  const versionedTx = await finalizeTransaction(connection, tx, seller)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Sell ${Number(tokenAmount) / 1e6} tokens for ${Number(result.solToUser) / 1e9} SOL${vaultLabel}`,
  }
}

// ============================================================================
// Create Token
// ============================================================================

/**
 * Build an unsigned create token transaction.
 *
 * Returns the transaction (partially signed by the mint keypair) and the mint keypair
 * so the agent can extract the mint address.
 *
 * @param connection - Solana RPC connection
 * @param params - Create parameters (creator, name, symbol, metadata_uri)
 * @returns Partially-signed transaction, mint PublicKey, and mint Keypair
 */
export const buildCreateTokenTransaction = async (
  connection: Connection,
  params: CreateTokenParams,
): Promise<CreateTokenResult> => {
  const {
    creator: creatorStr,
    name,
    symbol,
    metadata_uri,
    sol_target = 0,
    community_token = true,
  } = params

  const creator = new PublicKey(creatorStr)

  if (name.length > 32) throw new Error('Name must be 32 characters or less')
  if (symbol.length > 10) throw new Error('Symbol must be 10 characters or less')

  // Grind for vanity "tm" suffix
  let mint: Keypair
  const maxAttempts = 500_000
  let attempts = 0
  while (true) {
    mint = Keypair.generate()
    attempts++
    if (mint.publicKey.toBase58().endsWith('tm')) break
    if (attempts >= maxAttempts) break
  }

  // Derive PDAs
  const [globalConfig] = getGlobalConfigPda()
  const [bondingCurve] = getBondingCurvePda(mint.publicKey)
  const [treasury] = getTokenTreasuryPda(mint.publicKey)
  const bondingCurveTokenAccount = getAssociatedTokenAddressSync(
    mint.publicKey,
    bondingCurve,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  const treasuryTokenAccount = getTreasuryTokenAccount(mint.publicKey, treasury)
  // [V27] Treasury lock PDA and its token ATA
  const [treasuryLock] = getTreasuryLockPda(mint.publicKey)
  const treasuryLockTokenAccount = getTreasuryLockTokenAccount(mint.publicKey, treasuryLock)

  const tx = new Transaction()

  const provider = makeDummyProvider(connection, creator)
  const program = new Program(idl as unknown, provider)

  const createIx = await program.methods
    .createToken({
      name,
      symbol,
      uri: metadata_uri,
      solTarget: new BN(sol_target),
      communityToken: community_token,
    })
    .accounts({
      creator,
      globalConfig,
      mint: mint.publicKey,
      bondingCurve,
      tokenVault: bondingCurveTokenAccount,
      treasury,
      treasuryTokenAccount,
      treasuryLock,
      treasuryLockTokenAccount,
      token2022Program: TOKEN_2022_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
      rent: SYSVAR_RENT_PUBKEY,
    })
    .instruction()

  tx.add(createIx)
  const versionedTx = await finalizeTransaction(connection, tx, creator)

  // Partially sign with mint keypair
  versionedTx.sign([mint])

  return {
    transaction: versionedTx,
    mint: mint.publicKey,
    mintKeypair: mint,
    message: `Create token "${name}" ($${symbol})`,
  }
}

/**
 * Build, simulate, and submit a create token via signAndSendTransaction.
 *
 * Phantom-friendly: simulates with sigVerify: false (mint keypair is already
 * partially signed), then hands the tx to the wallet for the creator signature.
 * Avoids the "malicious dapp" warning caused by Phantom trying to simulate a
 * partially-signed transaction.
 *
 * @returns { signature, mint } on success
 */
export const sendCreateToken = async (
  connection: Connection,
  wallet: WalletAdapter,
  params: Omit<CreateTokenParams, 'creator'>,
): Promise<{ signature: string; mint: PublicKey }> => {
  const fullParams: CreateTokenParams = { ...params, creator: wallet.publicKey.toBase58() }
  const { transaction, mint } = await buildCreateTokenTransaction(connection, fullParams)

  const sim = await connection.simulateTransaction(transaction, { sigVerify: false })
  if (sim.value.err) {
    throw new Error(`Create token simulation failed: ${JSON.stringify(sim.value.err)}`)
  }

  const { signature } = await wallet.signAndSendTransaction(transaction)
  return { signature, mint }
}

// ============================================================================
// Star
// ============================================================================

/**
 * Build an unsigned star transaction (costs 0.05 SOL).
 *
 * @param connection - Solana RPC connection
 * @param params - Star parameters (mint, user)
 * @returns Unsigned transaction and descriptive message
 */
export const buildStarTransaction = async (
  connection: Connection,
  params: StarParams,
): Promise<TransactionResult> => {
  const { mint: mintStr, user: userStr, vault: vaultCreatorStr } = params

  const mint = new PublicKey(mintStr)
  const user = new PublicKey(userStr)

  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) throw new Error(`Token not found: ${mintStr}`)

  const { bondingCurve } = tokenData

  if (user.equals(bondingCurve.creator)) {
    throw new Error('Cannot star your own token')
  }

  // Check if already starred
  const [starRecordPda] = getStarRecordPda(user, mint)
  const starRecord = await connection.getAccountInfo(starRecordPda)
  if (starRecord) throw new Error('Already starred this token')

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)

  // Vault accounts (optional — vault pays star cost)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    user,
  )

  const tx = new Transaction()

  const provider = makeDummyProvider(connection, user)
  const program = new Program(idl as unknown, provider)

  const starBaseAccounts = {
    user,
    mint,
    bondingCurve: bondingCurvePda,
    tokenTreasury: treasuryPda,
    creator: bondingCurve.creator,
    starRecord: starRecordPda,
    systemProgram: SystemProgram.programId,
  }

  const starIx =
    torchVaultAccount && vaultWalletLinkAccount
      ? await program.methods
          .starTokenViaVault()
          .accounts({
            ...starBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
          })
          .instruction()
      : await program.methods.starToken().accounts(starBaseAccounts).instruction()

  tx.add(starIx)
  const versionedTx = await finalizeTransaction(connection, tx, user)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Star token (costs 0.05 SOL)${vaultLabel}`,
  }
}

// ============================================================================
// Message
// ============================================================================

// ============================================================================
// Vault (V2.0)
// ============================================================================

/**
 * Build an unsigned create vault transaction.
 *
 * Creates a TorchVault PDA and auto-links the creator's wallet.
 *
 * @param connection - Solana RPC connection
 * @param params - Creator public key
 * @returns Unsigned transaction
 */
export const buildCreateVaultTransaction = async (
  connection: Connection,
  params: CreateVaultParams,
): Promise<TransactionResult> => {
  const creator = new PublicKey(params.creator)
  const [vaultPda] = getTorchVaultPda(creator)
  const [walletLinkPda] = getVaultWalletLinkPda(creator)

  const provider = makeDummyProvider(connection, creator)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .createVault()
    .accounts({
      creator,
      vault: vaultPda,
      walletLink: walletLinkPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, creator)

  return {
    transaction: versionedTx,
    message: `Create vault for ${params.creator.slice(0, 8)}...`,
  }
}

/**
 * Build an unsigned deposit vault transaction.
 *
 * Anyone can deposit SOL into any vault.
 *
 * @param connection - Solana RPC connection
 * @param params - Depositor, vault creator, amount in lamports
 * @returns Unsigned transaction
 */
export const buildDepositVaultTransaction = async (
  connection: Connection,
  params: DepositVaultParams,
): Promise<TransactionResult> => {
  const depositor = new PublicKey(params.depositor)
  const vaultCreator = new PublicKey(params.vault_creator)
  const [vaultPda] = getTorchVaultPda(vaultCreator)

  const provider = makeDummyProvider(connection, depositor)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .depositVault(new BN(params.amount_sol.toString()))
    .accounts({
      depositor,
      vault: vaultPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, depositor)

  return {
    transaction: versionedTx,
    message: `Deposit ${params.amount_sol / 1e9} SOL into vault`,
  }
}

/**
 * Build an unsigned withdraw vault transaction.
 *
 * Only the vault authority can withdraw.
 *
 * @param connection - Solana RPC connection
 * @param params - Authority, vault creator, amount in lamports
 * @returns Unsigned transaction
 */
export const buildWithdrawVaultTransaction = async (
  connection: Connection,
  params: WithdrawVaultParams,
): Promise<TransactionResult> => {
  const authority = new PublicKey(params.authority)
  const vaultCreator = new PublicKey(params.vault_creator)
  const [vaultPda] = getTorchVaultPda(vaultCreator)

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .withdrawVault(new BN(params.amount_sol.toString()))
    .accounts({
      authority,
      vault: vaultPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Withdraw ${params.amount_sol / 1e9} SOL from vault`,
  }
}

/**
 * Build an unsigned link wallet transaction.
 *
 * Only the vault authority can link wallets.
 *
 * @param connection - Solana RPC connection
 * @param params - Authority, vault creator, wallet to link
 * @returns Unsigned transaction
 */
export const buildLinkWalletTransaction = async (
  connection: Connection,
  params: LinkWalletParams,
): Promise<TransactionResult> => {
  const authority = new PublicKey(params.authority)
  const vaultCreator = new PublicKey(params.vault_creator)
  const walletToLink = new PublicKey(params.wallet_to_link)
  const [vaultPda] = getTorchVaultPda(vaultCreator)
  const [walletLinkPda] = getVaultWalletLinkPda(walletToLink)

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .linkWallet()
    .accounts({
      authority,
      vault: vaultPda,
      walletToLink,
      walletLink: walletLinkPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Link wallet ${params.wallet_to_link.slice(0, 8)}... to vault`,
  }
}

/**
 * Build an unsigned unlink wallet transaction.
 *
 * Only the vault authority can unlink wallets. Rent returns to authority.
 *
 * @param connection - Solana RPC connection
 * @param params - Authority, vault creator, wallet to unlink
 * @returns Unsigned transaction
 */
export const buildUnlinkWalletTransaction = async (
  connection: Connection,
  params: UnlinkWalletParams,
): Promise<TransactionResult> => {
  const authority = new PublicKey(params.authority)
  const vaultCreator = new PublicKey(params.vault_creator)
  const walletToUnlink = new PublicKey(params.wallet_to_unlink)
  const [vaultPda] = getTorchVaultPda(vaultCreator)
  const [walletLinkPda] = getVaultWalletLinkPda(walletToUnlink)

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .unlinkWallet()
    .accounts({
      authority,
      vault: vaultPda,
      walletToUnlink,
      walletLink: walletLinkPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Unlink wallet ${params.wallet_to_unlink.slice(0, 8)}... from vault`,
  }
}

/**
 * Build an unsigned transfer authority transaction.
 *
 * Transfers vault admin control to a new wallet.
 *
 * @param connection - Solana RPC connection
 * @param params - Current authority, vault creator, new authority
 * @returns Unsigned transaction
 */
export const buildTransferAuthorityTransaction = async (
  connection: Connection,
  params: TransferAuthorityParams,
): Promise<TransactionResult> => {
  const authority = new PublicKey(params.authority)
  const vaultCreator = new PublicKey(params.vault_creator)
  const newAuthority = new PublicKey(params.new_authority)
  const [vaultPda] = getTorchVaultPda(vaultCreator)

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .transferAuthority()
    .accounts({
      authority,
      vault: vaultPda,
      newAuthority,
    })
    .instruction()

  const tx = new Transaction().add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Transfer vault authority to ${params.new_authority.slice(0, 8)}...`,
  }
}

// ============================================================================
// Borrow (V2.4)
// ============================================================================

/**
 * Build an unsigned borrow transaction.
 *
 * Lock tokens as collateral in the collateral vault and receive SOL from treasury.
 * Token must be migrated (has DeepPool for price calculation).
 *
 * @param connection - Solana RPC connection
 * @param params - Borrow parameters (mint, borrower, collateral_amount, sol_to_borrow)
 * @returns Unsigned transaction and descriptive message
 */
export const buildBorrowTransaction = async (
  connection: Connection,
  params: BorrowParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    borrower: borrowerStr,
    collateral_amount,
    sol_to_borrow,
    vault: vaultCreatorStr,
  } = params

  const mint = new PublicKey(mintStr)
  const borrower = new PublicKey(borrowerStr)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [collateralVaultPda] = getCollateralVaultPda(mint)
  const [loanPositionPda] = getLoanPositionPda(mint, borrower)

  const borrowerTokenAccount = getAssociatedTokenAddressSync(
    mint,
    borrower,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // DeepPool accounts for price calculation
  const deepPool = getDeepPoolAccounts(mint)

  // Vault accounts (optional — collateral from vault ATA, SOL to vault)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    borrower,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // borrower_token_account is mut + non-optional in the borrow instruction,
  // so the on-chain handler requires it to exist even in vault mode (collateral
  // flows vault → borrower ATA → collateral_vault).
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      borrower,
      borrowerTokenAccount,
      borrower,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(borrower, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, borrower)
  const program = new Program(idl as unknown, provider)

  const borrowArgs = {
    collateralAmount: new BN(collateral_amount.toString()),
    solToBorrow: new BN(sol_to_borrow.toString()),
  }
  const borrowBaseAccounts = {
    borrower,
    mint,
    bondingCurve: bondingCurvePda,
    treasury: treasuryPda,
    collateralVault: collateralVaultPda,
    borrowerTokenAccount,
    loanPosition: loanPositionPda,
    deepPool: deepPool.pool,
    deepPoolTokenVault: deepPool.tokenVault,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const borrowIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .borrowViaVault(borrowArgs)
          .accounts({
            ...borrowBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods.borrow(borrowArgs).accounts(borrowBaseAccounts).instruction()

  tx.add(borrowIx)
  const versionedTx = await finalizeTransaction(connection, tx, borrower)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Borrow ${Number(sol_to_borrow) / 1e9} SOL with ${Number(collateral_amount) / 1e6} tokens as collateral${vaultLabel}`,
  }
}

// ============================================================================
// Repay (V2.4)
// ============================================================================

/**
 * Build an unsigned repay transaction.
 *
 * Repay SOL debt. Interest is paid first, then principal.
 * Full repay returns all collateral and closes the position.
 *
 * @param connection - Solana RPC connection
 * @param params - Repay parameters (mint, borrower, sol_amount)
 * @returns Unsigned transaction and descriptive message
 */
export const buildRepayTransaction = async (
  connection: Connection,
  params: RepayParams,
): Promise<TransactionResult> => {
  const { mint: mintStr, borrower: borrowerStr, sol_amount, vault: vaultCreatorStr } = params

  const mint = new PublicKey(mintStr)
  const borrower = new PublicKey(borrowerStr)

  // Derive PDAs
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [collateralVaultPda] = getCollateralVaultPda(mint)
  const [loanPositionPda] = getLoanPositionPda(mint, borrower)

  const borrowerTokenAccount = getAssociatedTokenAddressSync(
    mint,
    borrower,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // Vault accounts (optional — SOL from vault, collateral returns to vault ATA)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    borrower,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // borrower_token_account is mut + non-optional; must exist even in vault mode.
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      borrower,
      borrowerTokenAccount,
      borrower,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(borrower, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, borrower)
  const program = new Program(idl as unknown, provider)

  const repayArg = new BN(sol_amount.toString())
  const repayBaseAccounts = {
    borrower,
    mint,
    treasury: treasuryPda,
    collateralVault: collateralVaultPda,
    borrowerTokenAccount,
    loanPosition: loanPositionPda,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const repayIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .repayViaVault(repayArg)
          .accounts({
            ...repayBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods.repay(repayArg).accounts(repayBaseAccounts).instruction()

  tx.add(repayIx)
  const versionedTx = await finalizeTransaction(connection, tx, borrower)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Repay ${Number(sol_amount) / 1e9} SOL${vaultLabel}`,
  }
}

// ============================================================================
// Liquidate (V2.4)
// ============================================================================

/**
 * Build an unsigned liquidate transaction.
 *
 * Permissionless — anyone can call when a borrower's LTV exceeds the
 * liquidation threshold. Liquidator pays SOL and receives collateral + bonus.
 *
 * @param connection - Solana RPC connection
 * @param params - Liquidate parameters (mint, liquidator, borrower)
 * @returns Unsigned transaction and descriptive message
 */
export const buildLiquidateTransaction = async (
  connection: Connection,
  params: LiquidateParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    liquidator: liquidatorStr,
    borrower: borrowerStr,
    vault: vaultCreatorStr,
  } = params

  const mint = new PublicKey(mintStr)
  const liquidator = new PublicKey(liquidatorStr)
  const borrower = new PublicKey(borrowerStr)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [collateralVaultPda] = getCollateralVaultPda(mint)
  const [loanPositionPda] = getLoanPositionPda(mint, borrower)

  const liquidatorTokenAccount = getAssociatedTokenAddressSync(
    mint,
    liquidator,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // DeepPool accounts for price calculation
  const deepPool = getDeepPoolAccounts(mint)

  // Vault accounts (optional — SOL from vault, collateral to vault ATA)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    liquidator,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // Create liquidator ATA if needed
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      liquidator,
      liquidatorTokenAccount,
      liquidator,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(liquidator, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, liquidator)
  const program = new Program(idl as unknown, provider)

  const liquidateSharedAccounts = {
    liquidator,
    borrower,
    mint,
    bondingCurve: bondingCurvePda,
    treasury: treasuryPda,
    collateralVault: collateralVaultPda,
    loanPosition: loanPositionPda,
    deepPool: deepPool.pool,
    deepPoolTokenVault: deepPool.tokenVault,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }
  const liquidateIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .liquidateViaVault()
          .accounts({
            ...liquidateSharedAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods
          .liquidate()
          .accounts({ ...liquidateSharedAccounts, liquidatorTokenAccount })
          .instruction()

  tx.add(liquidateIx)
  const versionedTx = await finalizeTransaction(connection, tx, liquidator)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Liquidate loan position for ${borrowerStr.slice(0, 8)}...${vaultLabel}`,
  }
}

// ============================================================================
// Claim Protocol Rewards
// ============================================================================

/**
 * Build an unsigned claim protocol rewards transaction.
 *
 * Claims the user's proportional share of protocol treasury rewards
 * based on trading volume in the previous epoch. Requires >= 2 SOL volume. Min claim: 0.1 SOL.
 *
 * @param connection - Solana RPC connection
 * @param params - Claim parameters (user, optional vault)
 * @returns Unsigned transaction and descriptive message
 */
export const buildClaimProtocolRewardsTransaction = async (
  connection: Connection,
  params: ClaimProtocolRewardsParams,
): Promise<TransactionResult> => {
  const { user: userStr, vault: vaultCreatorStr } = params

  const user = new PublicKey(userStr)

  // Derive PDAs
  const [userStatsPda] = getUserStatsPda(user)
  const [protocolTreasuryPda] = getProtocolTreasuryPda()

  // Vault accounts (optional — rewards go to vault instead of user)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    user,
  )

  const tx = new Transaction()

  const provider = makeDummyProvider(connection, user)
  const program = new Program(idl as unknown, provider)

  const claimBaseAccounts = {
    user,
    userStats: userStatsPda,
    protocolTreasury: protocolTreasuryPda,
    systemProgram: SystemProgram.programId,
  }

  const claimIx =
    torchVaultAccount && vaultWalletLinkAccount
      ? await program.methods
          .claimProtocolRewardsViaVault()
          .accounts({
            ...claimBaseAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
          })
          .instruction()
      : await program.methods
          .claimProtocolRewards()
          .accounts(claimBaseAccounts)
          .instruction()

  tx.add(claimIx)
  const versionedTx = await finalizeTransaction(connection, tx, user)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Claim protocol rewards${vaultLabel}`,
  }
}

// ============================================================================
// Reclaim Failed Token (V4)
// ============================================================================

/**
 * Build an unsigned reclaim-failed-token transaction.
 *
 * Permissionless — anyone can reclaim a failed token that has been
 * inactive for 7+ days and hasn't completed bonding.
 * SOL from both bonding curve and token treasury goes to protocol treasury.
 */
export const buildReclaimFailedTokenTransaction = async (
  connection: Connection,
  params: ReclaimParams,
): Promise<TransactionResult> => {
  const { payer: payerStr, mint: mintStr } = params
  const payer = new PublicKey(payerStr)
  const mint = new PublicKey(mintStr)

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [tokenTreasuryPda] = getTokenTreasuryPda(mint)
  const [protocolTreasuryPda] = getProtocolTreasuryPda()

  const tx = new Transaction()
  const provider = makeDummyProvider(connection, payer)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .reclaimFailedToken()
    .accounts({
      payer,
      mint,
      bondingCurve: bondingCurvePda,
      tokenTreasury: tokenTreasuryPda,
      protocolTreasury: protocolTreasuryPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  tx.add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, payer)

  return {
    transaction: versionedTx,
    message: `Reclaim failed token ${mintStr.slice(0, 8)}...`,
  }
}

// ============================================================================
// Withdraw Tokens (V18)
// ============================================================================

/**
 * Build an unsigned withdraw tokens transaction.
 *
 * Withdraw tokens from a vault ATA to any destination token account.
 * Authority only. Composability escape hatch for external DeFi.
 *
 * @param connection - Solana RPC connection
 * @param params - Authority, vault creator, mint, destination, amount in raw units
 * @returns Unsigned transaction
 */
export const buildWithdrawTokensTransaction = async (
  connection: Connection,
  params: WithdrawTokensParams,
): Promise<TransactionResult> => {
  const authority = new PublicKey(params.authority)
  const vaultCreator = new PublicKey(params.vault_creator)
  const mint = new PublicKey(params.mint)
  const destination = new PublicKey(params.destination)

  const [vaultPda] = getTorchVaultPda(vaultCreator)

  const vaultTokenAccount = getAssociatedTokenAddressSync(
    mint,
    vaultPda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )
  const destinationTokenAccount = getAssociatedTokenAddressSync(
    mint,
    destination,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  const tx = new Transaction()

  // Create destination ATA if needed
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      authority,
      destinationTokenAccount,
      destination,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .withdrawTokens(new BN(params.amount.toString()))
    .accounts({
      authority,
      vault: vaultPda,
      mint,
      vaultTokenAccount,
      destinationTokenAccount,
      tokenProgram: TOKEN_2022_PROGRAM_ID,
    })
    .instruction()

  tx.add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Withdraw ${params.amount} tokens from vault to ${params.destination.slice(0, 8)}...`,
  }
}

// ============================================================================
// Vault Swap (V19)
// ============================================================================

// ============================================================================
// Migration (V26)
// ============================================================================

// Build an unsigned migration transaction.
// Permissionless — anyone can call once bonding completes. Creates a DeepPool
// with locked liquidity (LP tokens burned). No WSOL wrapping — DeepPool uses
// native SOL. Payer fronts ~0.003 SOL for account rent, reimbursed by treasury
// in the same tx. When a buy completes bonding, buildBuyTransaction returns a
// separate migrationTransaction — the caller is expected to send both. Call
// buildMigrateTransaction directly only if that second tx was never sent.
export const buildMigrateTransaction = async (
  connection: Connection,
  params: MigrateParams,
): Promise<TransactionResult> => {
  const { mint: mintStr, payer: payerStr } = params

  const mint = new PublicKey(mintStr)
  const payer = new PublicKey(payerStr)

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [globalConfigPda] = getGlobalConfigPda()
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const treasuryTokenAccount = getTreasuryTokenAccount(mint, treasuryPda)
  const [treasuryLock] = getTreasuryLockPda(mint)
  const treasuryLockTokenAccount = getTreasuryLockTokenAccount(mint, treasuryLock)

  // Token vault = bonding curve's Token-2022 ATA
  const tokenVault = getAssociatedTokenAddressSync(
    mint,
    bondingCurvePda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )

  // Payer's token ATA (Token-2022)
  const payerToken = getAssociatedTokenAddressSync(mint, payer, false, TOKEN_2022_PROGRAM_ID)

  // DeepPool accounts
  const deepPool = getDeepPoolAccounts(mint)
  // Payer's LP ATA (Token-2022 — DeepPool LP mints are Token-2022)
  const payerLpAccount = getAssociatedTokenAddressSync(
    deepPool.lpMint,
    payer,
    false,
    TOKEN_2022_PROGRAM_ID,
  )
  // Pool PDA's LP ATA — receives locked LP (20% for creator)
  const deepPoolLpAccount = getAssociatedTokenAddressSync(
    deepPool.lpMint,
    deepPool.pool,
    true,
    TOKEN_2022_PROGRAM_ID,
  )

  const tx = new Transaction()

  // Compute budget — migration is heavy
  tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }))

  // Create payer's token ATA
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      payer,
      payerToken,
      payer,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  // Build program instructions
  const provider = makeDummyProvider(connection, payer)
  const program = new Program(idl as unknown, provider)

  // Step 1: Fund payer with bonding curve SOL (direct lamport manipulation, no CPI)
  const fundIx = await program.methods
    .fundMigrationSol()
    .accounts({
      payer,
      mint,
      bondingCurve: bondingCurvePda,
    })
    .instruction()

  // Step 2: Migrate to DeepPool (all CPI-based)
  const migrateIx = await program.methods
    .migrateToDex()
    .accounts({
      payer,
      globalConfig: globalConfigPda,
      mint,
      bondingCurve: bondingCurvePda,
      treasury: treasuryPda,
      tokenVault,
      treasuryTokenAccount,
      treasuryLockTokenAccount,
      treasuryLock,
      payerToken,
      deepPoolProgram: DEEP_POOL_PROGRAM_ID,
      torchConfig: deepPool.config,
      deepPool: deepPool.pool,
      deepPoolTokenVault: deepPool.tokenVault,
      deepPoolLpMint: deepPool.lpMint,
      payerLpAccount,
      deepPoolLpAccount,
      deepPoolEventAuthority: deepPool.eventAuthority,
      tokenProgram: TOKEN_PROGRAM_ID,
      token2022Program: TOKEN_2022_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  tx.add(fundIx, migrateIx)
  const versionedTx = await finalizeTransaction(connection, tx, payer)

  return {
    transaction: versionedTx,
    message: `Migrate token ${mintStr.slice(0, 8)}... to DeepPool`,
  }
}

// ============================================================================
// Vault Swap (V19)
// ============================================================================

/**
 * Build an unsigned vault-routed DEX swap transaction.
 *
 * Executes a DeepPool swap through the vault PDA for migrated Torch tokens.
 * Full custody preserved — all value flows through the vault.
 * No WSOL wrapping — DeepPool uses native SOL.
 *
 * @param connection - Solana RPC connection
 * @param params - Swap parameters (mint, signer, vault_creator, amount_in, minimum_amount_out, is_buy)
 * @returns Unsigned transaction and descriptive message
 */
const buildVaultSwapTransaction = async (
  connection: Connection,
  params: VaultSwapParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    signer: signerStr,
    vault_creator: vaultCreatorStr,
    amount_in,
    minimum_amount_out,
    is_buy,
    message,
  } = params

  const mint = new PublicKey(mintStr)
  const signer = new PublicKey(signerStr)
  const vaultCreator = new PublicKey(vaultCreatorStr)

  // Derive vault PDAs
  const [torchVaultPda] = getTorchVaultPda(vaultCreator)
  const [vaultSolPda] = getVaultSolPda(vaultCreator)
  const [vaultWalletLinkPda] = getVaultWalletLinkPda(signer)
  const [bondingCurvePda] = getBondingCurvePda(mint)

  // Vault's token ATA (Token-2022)
  const vaultTokenAccount = getAssociatedTokenAddressSync(
    mint,
    torchVaultPda,
    true,
    TOKEN_2022_PROGRAM_ID,
  )

  // DeepPool accounts
  const deepPool = getDeepPoolAccounts(mint)

  const tx = new Transaction()

  // Create vault token ATA if needed (for first buy of a migrated token)
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      signer,
      vaultTokenAccount,
      torchVaultPda,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  const provider = makeDummyProvider(connection, signer)
  const program = new Program(idl as unknown, provider)

  const swapIx = await program.methods
    .vaultSwap(new BN(amount_in.toString()), new BN(minimum_amount_out.toString()), is_buy)
    .accounts({
      signer,
      torchVault: torchVaultPda,
      vaultSol: vaultSolPda,
      vaultWalletLink: vaultWalletLinkPda,
      mint,
      bondingCurve: bondingCurvePda,
      vaultTokenAccount,
      deepPoolProgram: DEEP_POOL_PROGRAM_ID,
      deepPool: deepPool.pool,
      deepPoolTokenVault: deepPool.tokenVault,
      deepPoolEventAuthority: deepPool.eventAuthority,
      token2022Program: TOKEN_2022_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  tx.add(swapIx)

  addMemoIx(tx, signer, message, 280)

  const versionedTx = await finalizeTransaction(connection, tx, signer)

  const direction = is_buy ? 'Buy' : 'Sell'
  const amountLabel = is_buy ? `${amount_in / 1e9} SOL` : `${amount_in / 1e6} tokens`

  return {
    transaction: versionedTx,
    message: `${direction} ${amountLabel} via vault DEX swap`,
  }
}

// ============================================================================
// Treasury Cranks
// ============================================================================

// permissionless crank — rolls the protocol epoch forward so the previous epoch's
// trading-volume-weighted rewards become claimable via buildClaimProtocolRewardsTransaction.
// safe to call repeatedly; the program no-ops if the current epoch hasn't elapsed.
export const buildAdvanceProtocolEpochTransaction = async (
  connection: Connection,
  params: AdvanceProtocolEpochParams,
): Promise<TransactionResult> => {
  const { payer: payerStr } = params
  const payer = new PublicKey(payerStr)
  const [protocolTreasuryPda] = getProtocolTreasuryPda()

  const provider = makeDummyProvider(connection, payer)
  const program = new Program(idl as unknown, provider)

  const ix = await program.methods
    .advanceProtocolEpoch()
    .accounts({ payer, protocolTreasury: protocolTreasuryPda })
    .instruction()

  const tx = new Transaction()
  tx.add(ix)
  const versionedTx = await finalizeTransaction(connection, tx, payer)

  return { transaction: versionedTx, message: 'Advance protocol epoch' }
}

/**
 * Build an unsigned harvest-fees transaction.
 *
 * Permissionless crank — harvests accumulated Token-2022 transfer fees
 * from token accounts into the mint, then withdraws from the mint into
 * the treasury's token account.
 *
 * If `params.sources` is provided, uses those accounts directly.
 * Otherwise auto-discovers token accounts with withheld fees.
 */
export const buildHarvestFeesTransaction = async (
  connection: Connection,
  params: HarvestFeesParams,
): Promise<TransactionResult> => {
  const { mint: mintStr, payer: payerStr, sources: sourcesStr } = params

  const mint = new PublicKey(mintStr)
  const payer = new PublicKey(payerStr)

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const treasuryTokenAccount = getTreasuryTokenAccount(mint, treasuryPda)

  // Discover source accounts with withheld transfer fees
  let sourceAccounts: PublicKey[]

  if (sourcesStr && sourcesStr.length > 0) {
    sourceAccounts = sourcesStr.map((s) => new PublicKey(s))
  } else {
    // Auto-discover: fetch largest token accounts and filter to those with withheld > 0
    try {
      const largestAccounts = await connection.getTokenLargestAccounts(mint, 'confirmed')
      const addresses = largestAccounts.value.map((a) => a.address)

      if (addresses.length > 0) {
        const accountInfos = await connection.getMultipleAccountsInfo(addresses)
        sourceAccounts = []

        for (let i = 0; i < addresses.length; i++) {
          const info = accountInfos[i]
          if (!info) continue
          try {
            const account = unpackAccount(addresses[i], info, TOKEN_2022_PROGRAM_ID)
            const feeAmount = getTransferFeeAmount(account)
            if (feeAmount && feeAmount.withheldAmount > BigInt(0)) {
              sourceAccounts.push(addresses[i])
            }
          } catch {
            // Not a Token-2022 account or can't decode — skip
          }
        }
      } else {
        sourceAccounts = []
      }
    } catch {
      // RPC doesn't support getTokenLargestAccounts — proceed without source accounts
      sourceAccounts = []
    }
  }

  const provider = makeDummyProvider(connection, payer)
  const program = new Program(idl as unknown, provider)

  const tx = new Transaction()

  // Scale compute budget: base 200k + 20k per source account (Token-2022 harvest CPI is expensive)
  const computeUnits = 200_000 + 20_000 * sourceAccounts.length
  tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: computeUnits }))

  const harvestIx = await program.methods
    .harvestFees()
    .accounts({
      payer,
      mint,
      bondingCurve: bondingCurvePda,
      tokenTreasury: treasuryPda,
      treasuryTokenAccount,
      token2022Program: TOKEN_2022_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
    })
    .remainingAccounts(
      sourceAccounts.map((pubkey) => ({
        pubkey,
        isSigner: false,
        isWritable: true,
      })),
    )
    .instruction()

  tx.add(harvestIx)
  const versionedTx = await finalizeTransaction(connection, tx, payer)

  return {
    transaction: versionedTx,
    message: `Harvest transfer fees for ${mintStr.slice(0, 8)}... (${sourceAccounts.length} source accounts)`,
  }
}

/** Max transaction size in bytes (Solana packet data limit) */
const PACKET_DATA_SIZE = 1232

/**
 * [V20] Harvest transfer fees AND swap them to SOL.
 *
 * Tries to bundle: harvest_fees + swap_fees_to_sol.
 * If the combined transaction exceeds the 1232-byte limit (many source accounts),
 * automatically splits into a harvest-only tx + swap-only tx via additionalTransactions.
 * Set harvest=false to skip harvest (if already harvested separately).
 */
export const buildSwapFeesToSolTransaction = async (
  connection: Connection,
  params: SwapFeesToSolParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    payer: payerStr,
    minimum_amount_out = 1,
    harvest = true,
    sources: sourcesStr,
  } = params

  const mint = new PublicKey(mintStr)
  const payer = new PublicKey(payerStr)

  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const treasuryTokenAccount = getTreasuryTokenAccount(mint, treasuryPda)

  // DeepPool accounts
  const deepPool = getDeepPoolAccounts(mint)

  const provider = makeDummyProvider(connection, payer)
  const program = new Program(idl as unknown, provider)

  // Fetch bonding curve to get creator address for fee split
  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) throw new Error(`Token not found: ${mintStr}`)
  const creator = tokenData.bondingCurve.creator

  // Helper: build the harvest instruction with given sources
  const buildHarvestIx = async (sources: PublicKey[]) => {
    return program.methods
      .harvestFees()
      .accounts({
        payer,
        mint,
        bondingCurve: bondingCurvePda,
        tokenTreasury: treasuryPda,
        treasuryTokenAccount,
        token2022Program: TOKEN_2022_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      })
      .remainingAccounts(
        sources.map((pubkey) => ({
          pubkey,
          isSigner: false,
          isWritable: true,
        })),
      )
      .instruction()
  }

  // Helper: build the swap instruction
  const buildSwapIx = async () => {
    return program.methods
      .swapFeesToSol(new BN(minimum_amount_out.toString()))
      .accounts({
        payer,
        mint,
        bondingCurve: bondingCurvePda,
        creator,
        treasury: treasuryPda,
        treasuryTokenAccount,
        deepPoolProgram: DEEP_POOL_PROGRAM_ID,
        deepPool: deepPool.pool,
        deepPoolTokenVault: deepPool.tokenVault,
        deepPoolEventAuthority: deepPool.eventAuthority,
        token2022Program: TOKEN_2022_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .instruction()
  }

  // Discover source accounts
  let sourceAccounts: PublicKey[] = []

  if (harvest) {
    if (sourcesStr && sourcesStr.length > 0) {
      sourceAccounts = sourcesStr.map((s) => new PublicKey(s))
    } else {
      try {
        const largestAccounts = await connection.getTokenLargestAccounts(mint, 'confirmed')
        const addresses = largestAccounts.value.map((a) => a.address)

        if (addresses.length > 0) {
          const accountInfos = await connection.getMultipleAccountsInfo(addresses)

          for (let i = 0; i < addresses.length; i++) {
            const info = accountInfos[i]
            if (!info) continue
            try {
              const account = unpackAccount(addresses[i], info, TOKEN_2022_PROGRAM_ID)
              const feeAmount = getTransferFeeAmount(account)
              if (feeAmount && feeAmount.withheldAmount > BigInt(0)) {
                sourceAccounts.push(addresses[i])
              }
            } catch {
              // Not a Token-2022 account or can't decode — skip
            }
          }
        }
      } catch {
        sourceAccounts = []
      }
    }
  }

  // Try combined transaction first
  const tx = new Transaction()
  tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }))

  if (harvest && sourceAccounts.length > 0) {
    tx.add(await buildHarvestIx(sourceAccounts))
  }
  tx.add(await buildSwapIx())
  const versionedTx = await finalizeTransaction(connection, tx, payer)

  // Check if it fits in a single transaction
  let fitsInSingleTx = false
  try {
    const serialized = versionedTx.serialize()
    fitsInSingleTx = serialized.length <= PACKET_DATA_SIZE
  } catch {
    // serialize() throws when tx exceeds size limit
  }

  if (fitsInSingleTx) {
    return {
      transaction: versionedTx,
      message: `Swap harvested fees to SOL for ${mintStr.slice(0, 8)}...${harvest ? ' (harvest + swap)' : ''}`,
    }
  }

  // Too large — split into harvest tx + swap-only tx
  const harvestTx = new Transaction()
  const computeUnits = 200_000 + 20_000 * sourceAccounts.length
  harvestTx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: computeUnits }))
  harvestTx.add(await buildHarvestIx(sourceAccounts))
  const versionedHarvestTx = await finalizeTransaction(connection, harvestTx, payer)

  const swapTx = new Transaction()
  swapTx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }))
  swapTx.add(await buildSwapIx())
  const versionedSwapTx = await finalizeTransaction(connection, swapTx, payer)

  return {
    transaction: versionedHarvestTx,
    additionalTransactions: [versionedSwapTx],
    message: `Harvest + swap fees to SOL for ${mintStr.slice(0, 8)}... (split: ${sourceAccounts.length} sources)`,
  }
}

// ============================================================================
// Open Short (V5)
// ============================================================================

/**
 * Build an unsigned open_short transaction.
 *
 * Post SOL collateral and borrow tokens from treasury.
 * Mirror of borrow: same LTV, same liquidation, opposite direction.
 *
 * @param connection - Solana RPC connection
 * @param params - Open short parameters (mint, shorter, sol_collateral, tokens_to_borrow)
 * @returns Unsigned transaction and descriptive message
 */
export const buildOpenShortTransaction = async (
  connection: Connection,
  params: OpenShortParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    shorter: shorterStr,
    sol_collateral,
    tokens_to_borrow,
    vault: vaultCreatorStr,
  } = params

  const mint = new PublicKey(mintStr)
  const shorter = new PublicKey(shorterStr)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [treasuryLockPda] = getTreasuryLockPda(mint)
  const treasuryLockTokenAccount = getTreasuryLockTokenAccount(mint, treasuryLockPda)
  const [shortConfigPda] = getShortConfigPda(mint)
  const [shortPositionPda] = getShortPositionPda(mint, shorter)

  const shorterTokenAccount = getAssociatedTokenAddressSync(
    mint,
    shorter,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // DeepPool accounts for price calculation
  const deepPool = getDeepPoolAccounts(mint)

  // Vault accounts (optional — SOL from vault, tokens to vault ATA)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    shorter,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // Create shorter's token ATA if needed (to receive borrowed tokens)
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      shorter,
      shorterTokenAccount,
      shorter,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(shorter, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, shorter)
  const program = new Program(idl as unknown, provider)

  const openShortArgs = {
    solCollateral: new BN(sol_collateral.toString()),
    tokensToBorrow: new BN(tokens_to_borrow.toString()),
  }
  const openShortSharedAccounts = {
    shorter,
    mint,
    bondingCurve: bondingCurvePda,
    treasury: treasuryPda,
    treasuryLock: treasuryLockPda,
    treasuryLockTokenAccount,
    shortConfig: shortConfigPda,
    shortPosition: shortPositionPda,
    deepPool: deepPool.pool,
    deepPoolTokenVault: deepPool.tokenVault,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const openShortIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .openShortViaVault(openShortArgs)
          .accounts({
            ...openShortSharedAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods
          .openShort(openShortArgs)
          .accounts({ ...openShortSharedAccounts, shorterTokenAccount })
          .instruction()

  tx.add(openShortIx)
  const versionedTx = await finalizeTransaction(connection, tx, shorter)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Open short: ${Number(tokens_to_borrow) / 1e6} tokens with ${Number(sol_collateral) / 1e9} SOL collateral${vaultLabel}`,
  }
}

// ============================================================================
// Close Short (V5)
// ============================================================================

/**
 * Build an unsigned close_short transaction.
 *
 * Return tokens to close or partially repay a short position.
 * Interest paid first (in tokens), then principal.
 * Full close returns all SOL collateral.
 *
 * @param connection - Solana RPC connection
 * @param params - Close short parameters (mint, shorter, token_amount)
 * @returns Unsigned transaction and descriptive message
 */
export const buildCloseShortTransaction = async (
  connection: Connection,
  params: CloseShortParams,
): Promise<TransactionResult> => {
  const { mint: mintStr, shorter: shorterStr, token_amount, vault: vaultCreatorStr } = params

  const mint = new PublicKey(mintStr)
  const shorter = new PublicKey(shorterStr)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [treasuryLockPda] = getTreasuryLockPda(mint)
  const treasuryLockTokenAccount = getTreasuryLockTokenAccount(mint, treasuryLockPda)
  const [shortConfigPda] = getShortConfigPda(mint)
  const [shortPositionPda] = getShortPositionPda(mint, shorter)

  const shorterTokenAccount = getAssociatedTokenAddressSync(
    mint,
    shorter,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // Vault accounts (optional — tokens from vault ATA, SOL to vault)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    shorter,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // shorter_token_account is mut + non-optional; must exist even in vault mode.
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      shorter,
      shorterTokenAccount,
      shorter,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(shorter, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, shorter)
  const program = new Program(idl as unknown, provider)

  const closeShortArg = new BN(token_amount.toString())
  const closeShortSharedAccounts = {
    shorter,
    mint,
    bondingCurve: bondingCurvePda,
    treasury: treasuryPda,
    treasuryLock: treasuryLockPda,
    treasuryLockTokenAccount,
    shortConfig: shortConfigPda,
    shortPosition: shortPositionPda,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const closeShortIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .closeShortViaVault(closeShortArg)
          .accounts({
            ...closeShortSharedAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods
          .closeShort(closeShortArg)
          .accounts({ ...closeShortSharedAccounts, shorterTokenAccount })
          .instruction()

  tx.add(closeShortIx)
  const versionedTx = await finalizeTransaction(connection, tx, shorter)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Close short: return ${Number(token_amount) / 1e6} tokens${vaultLabel}`,
  }
}

// ============================================================================
// Liquidate Short (V5)
// ============================================================================

/**
 * Build an unsigned liquidate_short transaction.
 *
 * Permissionless — anyone can call when a short position's LTV exceeds the
 * liquidation threshold (65%). Liquidator sends tokens and receives SOL + bonus.
 *
 * @param connection - Solana RPC connection
 * @param params - Liquidate short parameters (mint, liquidator, borrower)
 * @returns Unsigned transaction and descriptive message
 */
export const buildLiquidateShortTransaction = async (
  connection: Connection,
  params: LiquidateShortParams,
): Promise<TransactionResult> => {
  const {
    mint: mintStr,
    liquidator: liquidatorStr,
    borrower: borrowerStr,
    vault: vaultCreatorStr,
  } = params

  const mint = new PublicKey(mintStr)
  const liquidator = new PublicKey(liquidatorStr)
  const borrower = new PublicKey(borrowerStr)

  // Derive PDAs
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [treasuryLockPda] = getTreasuryLockPda(mint)
  const treasuryLockTokenAccount = getTreasuryLockTokenAccount(mint, treasuryLockPda)
  const [shortConfigPda] = getShortConfigPda(mint)
  const [shortPositionPda] = getShortPositionPda(mint, borrower)

  const liquidatorTokenAccount = getAssociatedTokenAddressSync(
    mint,
    liquidator,
    false,
    TOKEN_2022_PROGRAM_ID,
  )

  // DeepPool accounts for price calculation
  const deepPool = getDeepPoolAccounts(mint)

  // Vault accounts (optional — tokens from vault ATA, SOL to vault)
  const { torchVault: torchVaultAccount, walletLink: vaultWalletLinkAccount } = deriveVaultAccounts(
    vaultCreatorStr,
    liquidator,
  )
  const vaultTokenAccount = torchVaultAccount ? getVaultTokenAta(mint, torchVaultAccount) : null

  const tx = new Transaction()

  // Create liquidator's token ATA if needed (source of covering tokens)
  tx.add(
    createAssociatedTokenAccountIdempotentInstruction(
      liquidator,
      liquidatorTokenAccount,
      liquidator,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID,
    ),
  )

  if (torchVaultAccount) {
    tx.add(createVaultTokenAtaIx(liquidator, mint, torchVaultAccount))
  }

  const provider = makeDummyProvider(connection, liquidator)
  const program = new Program(idl as unknown, provider)

  const liquidateShortSharedAccounts = {
    liquidator,
    borrower,
    mint,
    bondingCurve: bondingCurvePda,
    treasury: treasuryPda,
    treasuryLock: treasuryLockPda,
    treasuryLockTokenAccount,
    shortConfig: shortConfigPda,
    shortPosition: shortPositionPda,
    deepPool: deepPool.pool,
    deepPoolTokenVault: deepPool.tokenVault,
    tokenProgram: TOKEN_2022_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  }

  const liquidateShortIx =
    torchVaultAccount && vaultWalletLinkAccount && vaultTokenAccount
      ? await program.methods
          .liquidateShortViaVault()
          .accounts({
            ...liquidateShortSharedAccounts,
            torchVault: torchVaultAccount,
            vaultWalletLink: vaultWalletLinkAccount,
            vaultTokenAccount,
          })
          .instruction()
      : await program.methods
          .liquidateShort()
          .accounts({ ...liquidateShortSharedAccounts, liquidatorTokenAccount })
          .instruction()

  tx.add(liquidateShortIx)
  const versionedTx = await finalizeTransaction(connection, tx, liquidator)

  const vaultLabel = vaultCreatorStr ? ' (via vault)' : ''
  return {
    transaction: versionedTx,
    message: `Liquidate short position for ${borrowerStr.slice(0, 8)}...${vaultLabel}`,
  }
}

// ============================================================================
// Enable Short Selling (V5) — Admin / Pre-V5 Tokens
// ============================================================================

/**
 * Build an unsigned enable_short_selling transaction.
 *
 * Admin-only. For pre-V5 tokens that weren't created with the short selling
 * sentinel. New tokens (V5+) have shorts auto-enabled at creation.
 *
 * @param connection - Solana RPC connection
 * @param params - Enable short selling parameters (authority, mint)
 * @returns Unsigned transaction and descriptive message
 */
export const buildEnableShortSellingTransaction = async (
  connection: Connection,
  params: EnableShortSellingParams,
): Promise<TransactionResult> => {
  const { authority: authorityStr, mint: mintStr } = params

  const authority = new PublicKey(authorityStr)
  const mint = new PublicKey(mintStr)

  // Derive PDAs
  const [globalConfigPda] = getGlobalConfigPda()
  const [bondingCurvePda] = getBondingCurvePda(mint)
  const [treasuryPda] = getTokenTreasuryPda(mint)
  const [shortConfigPda] = getShortConfigPda(mint)

  const provider = makeDummyProvider(connection, authority)
  const program = new Program(idl as unknown, provider)

  const enableIx = await program.methods
    .enableShortSelling()
    .accounts({
      authority,
      globalConfig: globalConfigPda,
      mint,
      bondingCurve: bondingCurvePda,
      treasury: treasuryPda,
      shortConfig: shortConfigPda,
      systemProgram: SystemProgram.programId,
    })
    .instruction()

  const tx = new Transaction()
  tx.add(enableIx)
  const versionedTx = await finalizeTransaction(connection, tx, authority)

  return {
    transaction: versionedTx,
    message: `Enable short selling for ${mintStr.slice(0, 8)}...`,
  }
}
