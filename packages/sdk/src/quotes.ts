/**
 * Quote Calculations
 *
 * get expected output for buy/sell operations.
 * works for both bonding curve tokens and migrated (DeepPool DEX) tokens.
 */
import { Connection, PublicKey } from '@solana/web3.js'
import {
  calculateTokensOut,
  calculateSolOut,
  calculatePrice,
  getDeepPoolAccounts,
  getDepthMaxLtvBps,
  maxDebtValueForDepth,
} from './program'
import { LAMPORTS_PER_SOL, TOKEN_MULTIPLIER, TOTAL_SUPPLY } from './constants'
import { fetchTokenRaw, getToken, getLendingInfo } from './tokens'
import { BuyQuoteResult, SellQuoteResult, BorrowQuoteResult } from './types'

// DeepPool trade fee: 0.25% (25 bps)
const DEEP_POOL_FEE_BPS = 25

// Fetch DeepPool reserves for a migrated token.
// SOL reserve = pool PDA lamports - rent exempt (rent derived from actual account size).
// Token reserve = vault token balance.
const fetchPoolReserves = async (
  connection: Connection,
  mint: PublicKey,
): Promise<{ solReserves: bigint; tokenReserves: bigint }> => {
  const deepPool = getDeepPoolAccounts(mint)
  const poolInfo = await connection.getAccountInfo(deepPool.pool)
  if (!poolInfo) throw new Error('DeepPool not found')
  const [vaultBalance, rentExempt] = await Promise.all([
    connection.getTokenAccountBalance(deepPool.tokenVault),
    connection.getMinimumBalanceForRentExemption(poolInfo.data.length),
  ])
  const solReserves = BigInt(poolInfo.lamports) - BigInt(rentExempt)
  const tokenReserves = BigInt(vaultBalance.value.amount)
  return { solReserves, tokenReserves }
}

// Inverse CPMM buy quote: minimum SOL (lamports) to receive at least `tokensOut`
// tokens GROSS out of the pool (before the Token-2022 transfer fee on the
// pool→recipient leg). Ceil-rounded so the result never under-delivers.
//   effectiveIn = tokensOut * solReserves / (tokenReserves - tokensOut)   [ceil]
//   solIn       = effectiveIn * 10000 / (10000 - poolFee)                 [ceil]
// Used to size the cover-token buy in a one-click short liquidation.
export const quoteSolInForTokensOut = async (
  connection: Connection,
  mintStr: string,
  tokensOut: number,
): Promise<number> => {
  const mint = new PublicKey(mintStr)
  const { solReserves, tokenReserves } = await fetchPoolReserves(connection, mint)
  const out = BigInt(Math.ceil(tokensOut))
  if (out <= 0n) return 0
  if (out >= tokenReserves) throw new Error('tokensOut exceeds pool depth')
  const effectiveIn = (solReserves * out + (tokenReserves - out) - 1n) / (tokenReserves - out)
  const feeDenom = BigInt(10000 - DEEP_POOL_FEE_BPS)
  const amountIn = (effectiveIn * 10000n + feeDenom - 1n) / feeDenom
  return Number(amountIn)
}

// CPMM swap calculation: constant product with fee.
// effective_input = input * (10000 - fee_bps) / 10000
// output = effective_input * reserve_out / (reserve_in + effective_input)
const cpmmSwap = (
  amountIn: bigint,
  reserveIn: bigint,
  reserveOut: bigint,
  feeBps: number = DEEP_POOL_FEE_BPS,
): bigint => {
  const effectiveInput = (amountIn * BigInt(10000 - feeBps)) / BigInt(10000)
  return (effectiveInput * reserveOut) / (reserveIn + effectiveInput)
}

// get a buy quote: how many tokens for a given SOL amount.
// works for both bonding curve and migrated (DeepPool) tokens.
export const getBuyQuote = async (
  connection: Connection,
  mintStr: string,
  amountSolLamports: number,
): Promise<BuyQuoteResult> => {
  const mint = new PublicKey(mintStr)
  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) {
    throw new Error(`Token not found: ${mintStr}`)
  }

  const { bondingCurve } = tokenData
  // migrated token — quote from DeepPool
  if (bondingCurve.bonding_complete) {
    const { solReserves, tokenReserves } = await fetchPoolReserves(connection, mint)
    const amountSol = BigInt(amountSolLamports)
    const tokensOut = cpmmSwap(amountSol, solReserves, tokenReserves)
    // price = sol per token (in lamports per base unit)
    const priceBefore = Number(solReserves) / Number(tokenReserves)
    const priceAfter = Number(solReserves + amountSol) / Number(tokenReserves - tokensOut)
    const priceImpact = ((priceAfter - priceBefore) / priceBefore) * 100
    // price in human-readable: SOL per display token (with 6 decimals)
    const pricePerTokenSol = (priceBefore * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
    const minOutput = (tokensOut * BigInt(99)) / BigInt(100)
    return {
      input_sol: Number(amountSol),
      output_tokens: Number(tokensOut),
      tokens_to_user: Number(tokensOut),
      protocol_fee_sol: 0,
      price_per_token_sol: pricePerTokenSol,
      price_impact_percent: priceImpact,
      min_output_tokens: Number(minOutput),
      source: 'dex',
    }
  }

  // bonding curve token — use bonding math
  const virtualSol = BigInt(bondingCurve.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bondingCurve.virtual_token_reserves.toString())
  const realSol = BigInt(bondingCurve.real_sol_reserves.toString())
  const bondingTarget = BigInt(bondingCurve.bonding_target.toString())
  const amountSol = BigInt(amountSolLamports)
  const result = calculateTokensOut(
    amountSol,
    virtualSol,
    virtualTokens,
    realSol,
    100,
    100,
    bondingTarget,
  )
  const priceBefore = calculatePrice(virtualSol, virtualTokens)
  const priceAfter = calculatePrice(
    virtualSol + result.solToCurve,
    virtualTokens - result.tokensOut,
  )
  const priceImpact = ((priceAfter - priceBefore) / priceBefore) * 100
  const minOutput = (result.tokensToUser * BigInt(99)) / BigInt(100)
  return {
    input_sol: Number(amountSol),
    output_tokens: Number(result.tokensOut),
    tokens_to_user: Number(result.tokensToUser),
    protocol_fee_sol: Number(result.protocolFee),
    price_per_token_sol: (priceBefore * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL,
    price_impact_percent: priceImpact,
    min_output_tokens: Number(minOutput),
    source: 'bonding',
  }
}

// get a sell quote: how much SOL for a given token amount.
// works for both bonding curve and migrated (DeepPool) tokens.
export const getSellQuote = async (
  connection: Connection,
  mintStr: string,
  amountTokens: number,
): Promise<SellQuoteResult> => {
  const mint = new PublicKey(mintStr)
  const tokenData = await fetchTokenRaw(connection, mint)
  if (!tokenData) {
    throw new Error(`Token not found: ${mintStr}`)
  }

  const { bondingCurve } = tokenData
  // migrated token — quote from DeepPool
  if (bondingCurve.bonding_complete) {
    const { solReserves, tokenReserves } = await fetchPoolReserves(connection, mint)
    const tokenAmount = BigInt(amountTokens)
    const solOut = cpmmSwap(tokenAmount, tokenReserves, solReserves)
    const priceBefore = Number(solReserves) / Number(tokenReserves)
    const priceAfter = Number(solReserves - solOut) / Number(tokenReserves + tokenAmount)
    const priceImpact = ((priceBefore - priceAfter) / priceBefore) * 100
    const pricePerTokenSol = (priceBefore * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
    const minOutput = (solOut * BigInt(99)) / BigInt(100)
    return {
      input_tokens: Number(tokenAmount),
      output_sol: Number(solOut),
      protocol_fee_sol: 0,
      price_per_token_sol: pricePerTokenSol,
      price_impact_percent: priceImpact,
      min_output_sol: Number(minOutput),
      source: 'dex',
    }
  }

  // bonding curve token — use bonding math
  const virtualSol = BigInt(bondingCurve.virtual_sol_reserves.toString())
  const virtualTokens = BigInt(bondingCurve.virtual_token_reserves.toString())
  const tokenAmount = BigInt(amountTokens)
  const result = calculateSolOut(tokenAmount, virtualSol, virtualTokens)
  const priceBefore = calculatePrice(virtualSol, virtualTokens)
  const priceAfter = calculatePrice(virtualSol - result.solOut, virtualTokens + tokenAmount)
  const priceImpact = ((priceBefore - priceAfter) / priceBefore) * 100
  const minOutput = (result.solToUser * BigInt(99)) / BigInt(100)
  return {
    input_tokens: Number(tokenAmount),
    output_sol: Number(result.solToUser),
    protocol_fee_sol: 0,
    price_per_token_sol: (priceBefore * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL,
    price_impact_percent: priceImpact,
    min_output_sol: Number(minOutput),
    source: 'bonding',
  }
}

// [V21] Open-long quote: the maximum SOL borrowable against a given token
// collateral amount on a migrated token. Maps to `open_long` (borrow SOL vs
// token collateral). collateralAmount is in token base units (6 decimals).
//
// V21 dropped the per-user formula cap and the lending-unlock gate; the on-chain
// clamp is LTV-bound × treasury lendable headroom (vault × utilization_cap −
// already lent to longs). The result is an estimate — the program re-clamps.
export const getBorrowQuote = async (
  connection: Connection,
  mintStr: string,
  collateralAmount: number,
): Promise<BorrowQuoteResult> => {
  const TRANSFER_FEE_BPS = 7
  const mint = new PublicKey(mintStr)
  const [lending, detail, pool] = await Promise.all([
    getLendingInfo(connection, mintStr),
    getToken(connection, mintStr),
    fetchPoolReserves(connection, mint),
  ])
  const pricePerToken = detail.price_sol
  const poolSol = Number(pool.solReserves) // live pool SOL depth (lamports)
  // Token-2022 transfer fee reduces the net collateral that actually lands.
  const netCollateral = collateralAmount * (1 - TRANSFER_FEE_BPS / 10000)
  const collateralDisplayTokens = netCollateral / TOKEN_MULTIPLIER
  const collateralValueSol = collateralDisplayTokens * pricePerToken * LAMPORTS_PER_SOL
  // 1. LTV-bound cap. [V21] Effective LTV is the depth CURVE, not the flat ceiling —
  //    min(get_depth_max_ltv_bps(pool_sol), treasury.max_ltv_bps).
  const effectiveMaxLtvBps = Math.min(getDepthMaxLtvBps(poolSol), lending.max_ltv_bps)
  const ltvMaxSol = collateralValueSol * (effectiveMaxLtvBps / 10000)
  // 2. Treasury lendable headroom.
  const maxLendableSol = (lending.treasury_sol_vault_lamports * lending.utilization_cap_bps) / 10000
  const poolAvailableSol = Math.max(0, maxLendableSol - lending.total_sol_lent_to_longs)
  // 3. [V21] Rail-2 size cap: debt value ≤ ρ_max of pool SOL.
  const sizeCapSol = maxDebtValueForDepth(poolSol)

  // Lending is also gated on a minimum pool depth (the curve returns 0 below the
  // 100-SOL floor → no leverage), captured by effectiveMaxLtvBps === 0.
  const maxBorrowSol = lending.lending_enabled && effectiveMaxLtvBps > 0
    ? Math.max(0, Math.min(ltvMaxSol, poolAvailableSol, sizeCapSol))
    : 0

  return {
    max_borrow_sol: Math.floor(maxBorrowSol),
    collateral_value_sol: Math.floor(collateralValueSol),
    ltv_max_sol: Math.floor(ltvMaxSol),
    pool_available_sol: Math.floor(poolAvailableSol),
    size_cap_sol: Math.floor(sizeCapSol),
    interest_rate_bps: lending.interest_rate_bps,
    max_ltv_bps: effectiveMaxLtvBps,
    liquidation_threshold_bps: lending.liquidation_threshold_bps,
  }
}
