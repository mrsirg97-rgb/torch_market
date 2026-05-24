/**
 * useToken Hook
 *
 * Fetches and subscribes to a single token's data using the torchsdk.
 */

import { useEffect, useState, useMemo, useCallback, useRef } from 'react'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { PublicKey } from '@solana/web3.js'
import { getAssociatedTokenAddressSync } from '@solana/spl-token'
import { BorshCoder } from '@coral-xyz/anchor'
import type { TokenDetail, BondingCurve } from 'torchsdk'
import {
  getTreasuryState,
  getLendingInfo,
  getTokenMetadata,
  getTreasuryLockPda,
  getDeepPoolAccounts,
  calculatePrice,
  calculateBondingProgress,
} from 'torchsdk'
import idl from 'torchsdk/dist/torch_market.json'
import {
  PROGRAM_ID,
  SIMNET_PROGRAM_ID,
  BONDING_CURVE_SEED,
  TREASURY_SEED,
  STAR_RECORD_SEED,
  USER_POSITION_SEED,
  LAMPORTS_PER_SOL as LSOL,
  TOKEN_MULTIPLIER as TMUL,
  TREASURY_LOCK_TOKENS,
  TOKEN_2022_PROGRAM_ID,
} from '@/lib/constants'
import { useNetwork } from '@/lib/NetworkContext'
import { fetchPriceHistory, fetchCombinedPriceHistory, type PricePoint } from '@/lib/trades'

const isDev = process.env.NODE_ENV === 'development'

/** Parse Token-2022 TLV extensions to find TransferFeeAmount withheld amount on a token account */
function readWithheldAmount(data: Buffer): bigint {
  if (data.length <= 166) return BigInt(0)
  let offset = 166 // Extensions start after 165 base bytes + 1 account type byte
  while (offset + 4 <= data.length) {
    const extType = data.readUInt16LE(offset)
    const extLen = data.readUInt16LE(offset + 2)
    if (extType === 2 && extLen === 8 && offset + 12 <= data.length) {
      return data.readBigUInt64LE(offset + 4)
    }
    if (extLen === 0 && extType === 0) break // End of extensions
    offset += 4 + extLen
  }
  return BigInt(0)
}

export interface TokenMetadata {
  image?: string
  description?: string
  twitter?: string
  telegram?: string
  website?: string
}

export interface TokenMessage {
  signature: string
  memo: string
  sender: string
  timestamp: number
  sender_verified?: boolean
  sender_trust_tier?: 'high' | 'medium' | 'low' | null
  sender_said_name?: string
  sender_badge_url?: string
}

export interface UseTokenResult {
  // Validity
  isValidMint: boolean
  mint: PublicKey | null
  mintAddress: string

  // PDAs (kept for backward compat with components that need them)
  bondingCurvePda: PublicKey | null
  treasuryTokenAccount: PublicKey | null
  tokenTreasuryPda: PublicKey | null

  // SDK token detail (null until loaded)
  tokenDetail: TokenDetail | null

  // User-specific data (not available from SDK)
  userTokenBalance: bigint
  hasUserPosition: boolean
  hasStarred: boolean

  // Metadata (from SDK's TokenDetail)
  metadata: TokenMetadata | null
  solPriceUsd: number | null

  // Messages
  messages: TokenMessage[]

  // Price history (real trades)
  priceHistory: PricePoint[]

  // Loading states
  loading: boolean
  starLoading: boolean

  // Derived values (from SDK's TokenDetail)
  name: string
  symbol: string
  progress: number
  priceInSol: number
  circulatingSupply: number
  marketCapSol: number
  marketCapLamports: bigint
  isMigrated: boolean
  isComplete: boolean
  isVoting: boolean
  creator: string

  // Treasury data (from SDK's TokenDetail)
  treasurySolBalance: number
  treasuryTokenBalance: number
  stars: number
  solRaised: number
  solTarget: number

  // Treasury lending data
  totalSolLent: number
  activeLoans: number
  reserveRatioBps: number
  utilizationCapBps: number

  // Token supply data
  tokensInCurve: number
  tokensBurned: number
  treasuryLockBalance: number
  poolTokenBalance: number // Live DeepPool token vault balance (migrated tokens only)
  poolSolBalanceRaw: bigint // Raw lamports in DeepPool pool PDA (for swap quotes)
  poolTokenBalanceRaw: bigint // Raw token units in DeepPool token vault (for swap quotes)
  mintWithheldFees: number // Token-2022 transfer fees pending harvest from mint

  // DeepPool PDAs (for price history)
  deepPoolPda: PublicKey | null
  deepPoolTokenVault: PublicKey | null

  // Actions
  fetchToken: () => Promise<void>
  fetchStarRecord: () => Promise<void>
  fetchMessages: () => Promise<void>
  setStarLoading: (loading: boolean) => void
  setHasStarred: (starred: boolean) => void
}

export function useToken(mintAddress: string): UseTokenResult {
  const { connection } = useConnection()
  const { isSimnet, isDevnet, lendingGateLamports } = useNetwork()
  const wallet = useWallet()

  // SDK data
  const [tokenDetail, setTokenDetail] = useState<TokenDetail | null>(null)
  const [loading, setLoading] = useState(true)

  // Treasury lock balance (V27 locked supply)
  const [treasuryLockBalance, setTreasuryLockBalance] = useState<number>(0)
  // Live DeepPool token vault balance (migrated tokens)
  const [poolTokenBalance, setPoolTokenBalance] = useState<number>(0)
  // Raw pool vault balances for swap quote calculation
  const [poolSolBalanceRaw, setPoolSolBalanceRaw] = useState<bigint>(BigInt(0))
  const [poolTokenBalanceRaw, setPoolTokenBalanceRaw] = useState<bigint>(BigInt(0))
  // Token-2022 transfer fees withheld on mint (pending harvest)
  const [mintWithheldFees, setMintWithheldFees] = useState<number>(0)
  // Treasury lending data
  const [totalSolLent, setTotalSolLent] = useState<number>(0)
  const [activeLoans, setActiveLoans] = useState<number>(0)
  const [reserveRatioBps, setReserveRatioBps] = useState<number>(3000)
  const [utilizationCapBps, setUtilizationCapBps] = useState<number>(5000)

  // User-specific data
  const [userTokenBalance, setUserTokenBalance] = useState<bigint>(BigInt(0))
  const [hasUserPosition, setHasUserPosition] = useState(false)

  // Messages
  const [messages, setMessages] = useState<TokenMessage[]>([])

  // Star state
  const [hasStarred, setHasStarred] = useState(false)
  const [starLoading, setStarLoading] = useState(false)

  // SOL price
  const [solPriceUsd, setSolPriceUsd] = useState<number | null>(null)

  // Price history from real trades
  const [priceHistory, setPriceHistory] = useState<PricePoint[]>([])

  // Validate mint address and derive PDAs
  const { mint, isValidMint, bondingCurvePda, tokenTreasuryPda, treasuryTokenAccount, treasuryLockPda, deepPool } =
    useMemo(() => {
      try {
        const mintPubkey = new PublicKey(mintAddress)
        const programId = isSimnet ? SIMNET_PROGRAM_ID : PROGRAM_ID
        const [bcPda] = PublicKey.findProgramAddressSync(
          [Buffer.from(BONDING_CURVE_SEED), mintPubkey.toBuffer()],
          programId,
        )
        const [ttPda] = PublicKey.findProgramAddressSync(
          [Buffer.from(TREASURY_SEED), mintPubkey.toBuffer()],
          programId,
        )
        const treasuryAta = getAssociatedTokenAddressSync(
          mintPubkey,
          ttPda,
          true,
          TOKEN_2022_PROGRAM_ID,
        )
        const [tlPda] = getTreasuryLockPda(mintPubkey)
        const pool = getDeepPoolAccounts(mintPubkey)
        return {
          mint: mintPubkey,
          isValidMint: true,
          bondingCurvePda: bcPda,
          tokenTreasuryPda: ttPda,
          treasuryTokenAccount: treasuryAta,
          treasuryLockPda: tlPda,
          deepPool: pool,
        }
      } catch {
        return {
          mint: null,
          isValidMint: false,
          bondingCurvePda: null,
          tokenTreasuryPda: null,
          treasuryTokenAccount: null,
          treasuryLockPda: null,
          deepPool: null,
        }
      }
    }, [mintAddress, isSimnet])

  // Fetch token data directly from on-chain accounts (avoids SDK's CoinGecko CORS issue)
  const fetchToken = useCallback(async () => {
    if (!isValidMint || !mint || !bondingCurvePda || !tokenTreasuryPda) return
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const coder = new BorshCoder(idl as any)

      // Treasury lock ATA (for locked supply display)
      const treasuryLockAta = treasuryLockPda
        ? getAssociatedTokenAddressSync(mint, treasuryLockPda, true, TOKEN_2022_PROGRAM_ID)
        : null

      const [bcAccount, mintAccount, treasuryLockAtaAccount, treasuryAtaAccount, treasury, lendingInfo, mintMeta] = await Promise.all([
        connection.getAccountInfo(bondingCurvePda),
        connection.getAccountInfo(mint),
        treasuryLockAta ? connection.getAccountInfo(treasuryLockAta) : Promise.resolve(null),
        treasuryTokenAccount ? connection.getAccountInfo(treasuryTokenAccount) : Promise.resolve(null),
        getTreasuryState(connection, mintAddress),
        getLendingInfo(connection, mintAddress, lendingGateLamports).catch(() => null),
        getTokenMetadata(connection, mintAddress).catch(() => null),
      ])
      if (!bcAccount) return

      const bc = coder.accounts.decode('BondingCurve', bcAccount.data) as unknown as BondingCurve

      // [v20] name/symbol/uri now live on the Token-2022 metadata extension.
      const tokenName = mintMeta?.name ?? ''
      const tokenSymbol = mintMeta?.symbol ?? ''
      const uri = mintMeta?.uri ?? ''

      // Read treasury lock token balance (u64 LE at offset 64 in token account)
      const treasuryLockTokens = treasuryLockAtaAccount && treasuryLockAtaAccount.data.length >= 72
        ? treasuryLockAtaAccount.data.readBigUInt64LE(64)
        : BigInt(0)

      // Fetch off-chain metadata JSON (image, description, socials) with 10s timeout
      let metadata: { description?: string; image?: string; twitter?: string; telegram?: string; website?: string } | undefined
      if (uri) {
        try {
          const controller = new AbortController()
          const timer = setTimeout(() => controller.abort(), 10_000)
          const res = await fetch(uri, { signal: controller.signal }).finally(() => clearTimeout(timer))
          const data = await res.json()
          metadata = {
            description: data.description,
            image: data.image,
            twitter: data.twitter,
            telegram: data.telegram,
            website: data.website,
          }
        } catch { /* metadata fetch failed, continue */ }
      }

      // Build TokenDetail
      const TOTAL_SUPPLY = BigInt('1000000000000000')
      const virtualSol = BigInt(bc.virtual_sol_reserves.toString())
      const virtualTokens = BigInt(bc.virtual_token_reserves.toString())
      const realSol = BigInt(bc.real_sol_reserves.toString())
      const realTokens = BigInt(bc.real_token_reserves.toString())
      // Use mint's actual supply (accounts for all burns including migration excess)
      // Token-2022 mint layout: supply is u64 LE at offset 36
      const mintSupply = mintAccount && mintAccount.data.length >= 44
        ? mintAccount.data.readBigUInt64LE(36)
        : TOTAL_SUPPLY
      const actualBurned = TOTAL_SUPPLY - mintSupply

      // For migrated tokens, fetch live DeepPool price.
      // DeepPool stores SOL on the pool PDA's lamports (minus rent-exempt
      // floor) and tokens in a Token-2022 vault.
      const isMigrated = bc.migrated
      let poolSolBalance = BigInt(0)
      let poolTokenBalance = BigInt(0)

      let poolVaultWithheld = BigInt(0)
      if (isMigrated && deepPool) {
        try {
          const [poolAccount, tokenVaultAccount] = await Promise.all([
            connection.getAccountInfo(deepPool.pool),
            connection.getAccountInfo(deepPool.tokenVault),
          ])
          if (poolAccount) {
            const rentExempt = await connection.getMinimumBalanceForRentExemption(poolAccount.data.length)
            const sol = BigInt(poolAccount.lamports) - BigInt(rentExempt)
            poolSolBalance = sol > BigInt(0) ? sol : BigInt(0)
          }
          // Token account layout: amount is u64 LE at offset 64
          if (tokenVaultAccount && tokenVaultAccount.data.length >= 72) {
            poolTokenBalance = tokenVaultAccount.data.readBigUInt64LE(64)
            // Read withheld transfer fees on pool token vault (biggest fee source from DEX trades)
            poolVaultWithheld = readWithheldAmount(tokenVaultAccount.data)
          }
        } catch { /* pool not found — fall back to baseline */ }
      }

      const hasLivePool = isMigrated && poolTokenBalance > BigInt(0) && poolSolBalance > BigInt(0)
      const baselineTokens = treasury ? BigInt(treasury.baseline_token_reserves) : BigInt(0)
      const baselineSol = treasury ? BigInt(treasury.baseline_sol_reserves) : BigInt(0)
      const hasPoolBaseline = isMigrated && baselineTokens > BigInt(0)

      const price = hasLivePool
        ? Number(poolSolBalance) / Number(poolTokenBalance)
        : hasPoolBaseline
          ? Number(baselineSol) / Number(baselineTokens)
          : calculatePrice(virtualSol, virtualTokens)
      const priceInSol = (price * TMUL) / LSOL

      // Circulating supply (for display) — vote_vault_balance is always 0 (removed in V36)
      const circulating = mintSupply - realTokens - TREASURY_LOCK_TOKENS
      // Market cap = fully diluted (total supply × price), matching pump.fun convention
      const marketCapSol = (priceInSol * Number(TOTAL_SUPPLY)) / TMUL
      const treasurySol = treasury?.sol_balance_sol ?? 0
      // Read actual token balance from treasury ATA (tokens_held accounting field is not updated by handlers)
      const treasuryAtaBalance = treasuryAtaAccount && treasuryAtaAccount.data.length >= 72
        ? treasuryAtaAccount.data.readBigUInt64LE(64)
        : BigInt(0)
      const treasuryTokens = Number(treasuryAtaBalance) / TMUL
      const stars = treasury?.total_stars ?? 0
      const totalSolLent = lendingInfo ? (lendingInfo.total_sol_lent ?? 0) / LSOL : 0
      const activeLoans = lendingInfo?.active_loans ?? 0
      const utilizationCapBps = lendingInfo?.utilization_cap_bps ?? 5000
      // reserve_ratio_bps isn't exposed by the SDK's LendingInfo; fall back to the historical default
      const reserveRatioBps = 3000

      const status: 'bonding' | 'complete' | 'migrated' = bc.migrated
        ? 'migrated'
        : bc.bonding_complete
          ? 'complete'
          : 'bonding'

      const detail: TokenDetail = {
        mint: mintAddress,
        name: tokenName,
        symbol: tokenSymbol,
        description: metadata?.description,
        image: metadata?.image,
        status,
        price_sol: priceInSol,
        price_usd: undefined,
        market_cap_sol: marketCapSol,
        market_cap_usd: undefined,
        progress_percent: calculateBondingProgress(realSol, BigInt(bc.bonding_target.toString())),
        sol_raised: bc.migrated
          ? Number(bc.bonding_target.toString()) / LSOL
          : Number(realSol) / LSOL,
        sol_target: Number(bc.bonding_target.toString()) / LSOL || 200,
        total_supply: Number(TOTAL_SUPPLY) / TMUL,
        circulating_supply: Number(circulating) / TMUL,
        tokens_in_curve: Number(realTokens) / TMUL,
        tokens_burned: Number(actualBurned) / TMUL,
        treasury_sol_balance: treasurySol,
        treasury_token_balance: treasuryTokens,
        creator: bc.creator.toString(),
        holders: null,
        stars,
        created_at: 0,
        last_activity_at: Number(bc.last_activity_slot.toString()),
        twitter: metadata?.twitter,
        telegram: metadata?.telegram,
        website: metadata?.website,
      }

      // Read Token-2022 transfer fee withheld amount from mint (offset 234 in TransferFeeConfig extension)
      const withheldOnMint = mintAccount && mintAccount.data.length >= 242
        ? mintAccount.data.readBigUInt64LE(234)
        : BigInt(0)
      // Read withheld on treasury ATA (from transfers)
      const treasuryAtaWithheld = treasuryAtaAccount
        ? readWithheldAmount(treasuryAtaAccount.data)
        : BigInt(0)

      setTreasuryLockBalance(Number(treasuryLockTokens) / TMUL)
      setPoolTokenBalance(Number(poolTokenBalance) / TMUL)
      setPoolSolBalanceRaw(poolSolBalance)
      setPoolTokenBalanceRaw(poolTokenBalance)
      // Harvestable = pool vault withheld (DEX trades) + treasury ATA withheld + mint withheld
      setMintWithheldFees(Number(poolVaultWithheld + treasuryAtaWithheld + withheldOnMint) / TMUL)
      setTotalSolLent(totalSolLent)
      setActiveLoans(activeLoans)
      setReserveRatioBps(reserveRatioBps)
      setUtilizationCapBps(utilizationCapBps)

      setTokenDetail((prev) => {
        if (!prev) return detail
        if (
          prev.price_sol !== detail.price_sol ||
          prev.status !== detail.status ||
          prev.progress_percent !== detail.progress_percent ||
          prev.sol_raised !== detail.sol_raised
        ) {
          return detail
        }
        return prev
      })
    } catch (err) {
      if (isDev) console.error('Error fetching token:', err)
    }
  }, [connection, mintAddress, isValidMint, mint, bondingCurvePda, tokenTreasuryPda, treasuryLockPda, deepPool])

  // Fetch user token balance
  const fetchUserBalance = useCallback(async () => {
    if (!mint || !wallet.publicKey) {
      setUserTokenBalance(BigInt(0))
      return
    }
    try {
      const userAta = getAssociatedTokenAddressSync(
        mint,
        wallet.publicKey,
        false,
        TOKEN_2022_PROGRAM_ID,
      )
      const ataAccount = await connection.getAccountInfo(userAta)
      if (ataAccount && ataAccount.data.length >= 72) {
        setUserTokenBalance(ataAccount.data.readBigUInt64LE(64))
      } else {
        setUserTokenBalance(BigInt(0))
      }
    } catch {
      setUserTokenBalance(BigInt(0))
    }
  }, [connection, mint, wallet.publicKey])

  // Fetch user position (checks if user has an on-chain position PDA for this token)
  const fetchUserPosition = useCallback(async () => {
    if (!wallet.publicKey || !bondingCurvePda) {
      setHasUserPosition(false)
      return
    }
    try {
      const programId = isSimnet ? SIMNET_PROGRAM_ID : PROGRAM_ID
      const [userPositionPda] = PublicKey.findProgramAddressSync(
        [Buffer.from(USER_POSITION_SEED), bondingCurvePda.toBuffer(), wallet.publicKey.toBuffer()],
        programId,
      )
      const account = await connection.getAccountInfo(userPositionPda)
      setHasUserPosition(!!account)
    } catch (err) {
      if (isDev) console.error('Error fetching user position:', err)
      setHasUserPosition(false)
    }
  }, [connection, wallet.publicKey, bondingCurvePda, isSimnet])

  // Fetch star record
  const fetchStarRecord = useCallback(async () => {
    if (!wallet.publicKey || !mint) {
      setHasStarred(false)
      return
    }
    try {
      const programId = isSimnet ? SIMNET_PROGRAM_ID : PROGRAM_ID
      const [starRecordPda] = PublicKey.findProgramAddressSync(
        [Buffer.from(STAR_RECORD_SEED), wallet.publicKey.toBuffer(), mint.toBuffer()],
        programId,
      )
      const starRecordAccount = await connection.getAccountInfo(starRecordPda)
      setHasStarred(!!starRecordAccount)
    } catch (err) {
      if (isDev) console.error('Error fetching star record:', err)
      setHasStarred(false)
    }
  }, [connection, wallet.publicKey, mint, isSimnet])

  // Fetch messages via server-side cached API
  const fetchMessages = useCallback(async () => {
    if (!isValidMint) return
    try {
      const res = await fetch(`/api/v1/messages/${mintAddress}`)
      if (!res.ok) throw new Error('Failed to fetch messages')
      const data = await res.json()
      setMessages(data.messages)
    } catch (err) {
      if (isDev) console.error('[Messages] Error fetching messages:', err)
    }
  }, [mintAddress, isValidMint])

  // Track if initial load is done
  const initialLoadDone = useRef(false)

  // Main data fetch and subscriptions
  useEffect(() => {
    if (!isValidMint || !mint || !bondingCurvePda || !tokenTreasuryPda) {
      setLoading(false)
      return
    }

    const subscriptions: number[] = []
    const programId = isSimnet ? SIMNET_PROGRAM_ID : PROGRAM_ID

    async function fetchData(isRefresh = false) {
      if (!isRefresh) setLoading(true)
      try {
        await Promise.all([
          fetchToken(),
          fetchUserBalance(),
          ...(!isRefresh ? [fetchStarRecord(), fetchUserPosition()] : []),
        ])
      } catch (err) {
        if (isDev) console.error('Error fetching data:', err)
      } finally {
        if (!isRefresh) {
          setLoading(false)
          initialLoadDone.current = true
        }
      }
    }

    fetchData(false)

    // On simnet, poll instead of subscribing
    if (isSimnet) {
      const pollInterval = setInterval(() => fetchData(true), 3000)
      return () => clearInterval(pollInterval)
    }

    // Subscribe to bonding curve changes → trigger SDK refetch
    const bcSub = connection.onAccountChange(
      bondingCurvePda,
      () => {
        fetchToken()
        fetchUserBalance()
        fetchUserPosition()
      },
      { commitment: 'confirmed' },
    )
    subscriptions.push(bcSub)

    // Subscribe to treasury changes → trigger SDK refetch
    const treasurySub = connection.onAccountChange(
      tokenTreasuryPda,
      () => {
        fetchToken()
      },
      { commitment: 'confirmed' },
    )
    subscriptions.push(treasurySub)

    // Subscribe to user token balance if wallet connected
    if (wallet.publicKey && mint) {
      const userAta = getAssociatedTokenAddressSync(
        mint,
        wallet.publicKey,
        false,
        TOKEN_2022_PROGRAM_ID,
      )
      const userAtaSub = connection.onAccountChange(
        userAta,
        (accountInfo) => {
          try {
            const data = accountInfo.data
            if (data.length >= 72) {
              setUserTokenBalance(data.readBigUInt64LE(64))
            }
          } catch {
            // Decode error
          }
        },
        { commitment: 'confirmed' },
      )
      subscriptions.push(userAtaSub)
    }

    return () => {
      subscriptions.forEach((sub) => connection.removeAccountChangeListener(sub))
    }
  }, [connection, wallet.publicKey, bondingCurvePda, tokenTreasuryPda, mint, isValidMint, isSimnet, fetchToken, fetchUserBalance, fetchStarRecord, fetchUserPosition])

  // Fetch SOL price in USD
  useEffect(() => {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), 5000)

    async function fetchSolPrice() {
      try {
        const res = await fetch('/api/sol-price', { signal: controller.signal })
        if (!res.ok) return
        const data = await res.json()
        if (typeof data?.solana?.usd === 'number' && data.solana.usd > 0) {
          setSolPriceUsd(data.solana.usd)
        }
      } catch {
        // Silently fail
      } finally {
        clearTimeout(timeoutId)
      }
    }
    fetchSolPrice()

    return () => {
      clearTimeout(timeoutId)
      controller.abort()
    }
  }, [])

  // Fetch price history from real transactions once token detail is loaded
  // Skip on devnet — public RPC rate limits make this impractical
  // For migrated tokens: fetch both bonding curve + Raydium pool history
  useEffect(() => {
    if (!isValidMint || !tokenDetail || isDevnet) return
    let cancelled = false

    const isMigratedToken = tokenDetail.status === 'migrated'

    if (isMigratedToken) {
      fetchCombinedPriceHistory(
        connection,
        mintAddress,
        tokenDetail.sol_raised,
        tokenDetail.sol_target,
      )
        .then((points) => {
          if (!cancelled) setPriceHistory(points)
        })
        .catch(() => {
          // Silently fail — chart shows loading state
        })
    } else {
      fetchPriceHistory(connection, mintAddress, tokenDetail.sol_raised, tokenDetail.sol_target)
        .then((points) => {
          if (!cancelled) setPriceHistory(points)
        })
        .catch(() => {
          // Silently fail — chart shows loading state
        })
    }

    return () => {
      cancelled = true
    }
  }, [isValidMint, connection, mintAddress, tokenDetail, isDevnet])

  // Fetch and refresh messages (deferred to avoid competing with initial data load)
  useEffect(() => {
    if (!isValidMint) return

    const deferTimeout = setTimeout(() => {
      fetchMessages()
    }, 3000)
    const interval = setInterval(fetchMessages, 60000)
    return () => {
      clearTimeout(deferTimeout)
      clearInterval(interval)
    }
  }, [isValidMint, fetchMessages])

  // Derived values from SDK's TokenDetail
  const derivedValues = useMemo(() => {
    if (!tokenDetail) {
      return {
        name: '',
        symbol: '',
        progress: 0,
        priceInSol: 0,
        circulatingSupply: 0,
        marketCapSol: 0,
        marketCapLamports: BigInt(0),
        isMigrated: false,
        isComplete: false,
        isVoting: false,
        creator: '',
        treasurySolBalance: 0,
        treasuryTokenBalance: 0,
        stars: 0,
        solRaised: 0,
        solTarget: 200,
        tokensInCurve: 0,
        tokensBurned: 0,
      }
    }

    const LAMPORTS_PER_SOL = 1_000_000_000
    const isMigrated = tokenDetail.status === 'migrated'
    // 'complete' in SDK means bonding_complete && !migrated (covers both voting and post-vote)
    const isComplete = tokenDetail.status === 'complete'
    // If bonding is complete but not migrated → voting phase
    const isVoting = isComplete

    return {
      name: tokenDetail.name,
      symbol: tokenDetail.symbol,
      progress: tokenDetail.progress_percent,
      priceInSol: tokenDetail.price_sol,
      circulatingSupply: tokenDetail.circulating_supply,
      marketCapSol: tokenDetail.market_cap_sol,
      marketCapLamports: BigInt(Math.floor(tokenDetail.market_cap_sol * LAMPORTS_PER_SOL)),
      isMigrated,
      isComplete,
      isVoting,
      creator: tokenDetail.creator,
      treasurySolBalance: tokenDetail.treasury_sol_balance,
      treasuryTokenBalance: tokenDetail.treasury_token_balance,
      stars: tokenDetail.stars,
      solRaised: tokenDetail.sol_raised,
      solTarget: tokenDetail.sol_target,
      tokensInCurve: tokenDetail.tokens_in_curve,
      tokensBurned: tokenDetail.tokens_burned,
    }
  }, [tokenDetail])

  // Metadata from TokenDetail
  const metadata = useMemo<TokenMetadata | null>(() => {
    if (!tokenDetail) return null
    if (!tokenDetail.image && !tokenDetail.description) return null
    return {
      image: tokenDetail.image,
      description: tokenDetail.description,
      twitter: tokenDetail.twitter,
      telegram: tokenDetail.telegram,
      website: tokenDetail.website,
    }
  }, [tokenDetail])

  return {
    isValidMint,
    mint,
    mintAddress,
    bondingCurvePda,
    treasuryTokenAccount,
    tokenTreasuryPda,
    tokenDetail,
    userTokenBalance,
    hasUserPosition,
    hasStarred,
    metadata,
    solPriceUsd,
    messages,
    priceHistory,
    loading,
    starLoading,
    ...derivedValues,
    treasuryLockBalance,
    poolTokenBalance,
    poolSolBalanceRaw,
    poolTokenBalanceRaw,
    mintWithheldFees,
    totalSolLent,
    activeLoans,
    reserveRatioBps,
    utilizationCapBps,
    deepPoolPda: deepPool?.pool ?? null,
    deepPoolTokenVault: deepPool?.tokenVault ?? null,
    fetchToken,
    fetchStarRecord,
    fetchMessages,
    setStarLoading,
    setHasStarred,
  }
}
