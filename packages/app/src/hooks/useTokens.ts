/**
 * useTokens Hook
 *
 * Fetches and subscribes to token data using the torchsdk.
 */

import { useEffect, useState, useMemo, useRef, useCallback } from 'react'
import { useConnection } from '@solana/wallet-adapter-react'
import { PublicKey } from '@solana/web3.js'
import { BorshCoder } from '@coral-xyz/anchor'
import {
  getTokens,
  getBondingCurvePda,
  getDeepPoolAccounts,
  fetchMintsMetadata,
  calculateBondingProgress,
} from 'torchsdk'
import type { BondingCurve } from 'torchsdk'
import idl from 'torchsdk/dist/torch_market.json'
import { PROGRAM_ID, SIMNET_PROGRAM_ID, LAMPORTS_PER_SOL, TOKEN_MULTIPLIER, TOTAL_SUPPLY } from '@/lib/constants'
import { TokenData, TokenEnrichment, TokenFilter, TokenTier, matchesFilter, getTierFromTarget } from '@/types/token'
import { useNetwork } from '@/lib/NetworkContext'
import { useTorchFeed, useFeedHealthy } from '@/lib/TorchFeedContext'

function isLikelyValidMetadataUri(uri: string): boolean {
  try {
    const u = new URL(uri)
    if (u.protocol !== 'https:') return false
    const host = u.hostname.toLowerCase()
    // Metadata lives on arweave. Irys hosts get rewritten to arweave.net at fetch time.
    return host === 'arweave.net' || host === 'gateway.irys.xyz' || host === 'uploader.irys.xyz'
  } catch {
    return false
  }
}

interface UseTokensResult {
  /** All tokens from the program */
  tokens: TokenData[]
  /** Whether initial load is in progress */
  loading: boolean
  /** [prompt-007] Unseen markets announced by the AllMarkets room (pin count) */
  pendingNew: number
  /** Pull pending new markets into the list + clear the pin */
  acceptNewMarkets: () => void
  /** Filter counts for each filter type */
  filterCounts: Record<TokenFilter, number>
  /** Top projects (highest market cap completed tokens) */
  topProjects: TokenData[]
  /** Trending tokens (75-100% progress, still bonding) */
  trendingTokens: TokenData[]
  /** Current slot for relative time display */
  currentSlot: bigint
  /** Tokens grouped by tier */
  tokensByTier: Record<TokenTier, TokenData[]>
  /** Count of tokens per tier */
  tierCounts: Record<TokenTier, number>
}

/**
 * Hook to fetch and subscribe to token data
 * @param options.enabled - Whether to fetch data (default: true). Set to false to defer the heavy getProgramAccounts call.
 */
export function useTokens(options?: { enabled?: boolean }): UseTokensResult {
  const enabled = options?.enabled !== false
  const { connection } = useConnection()
  const { isSimnet, isMainnet, effectiveIndexerUrl } = useNetwork()
  const [loading, setLoading] = useState(true)
  const [rawTokens, setRawTokens] = useState<TokenData[]>([])
  const feedHealthy = useFeedHealthy()
  // [prompt-007] AllMarkets frames, handled calmly:
  //   - market frame for an UNKNOWN mint → count it for the "new markets" pin
  //     (no auto-refetch; the list never reorders under the user's cursor)
  //   - trade/swap ticks + known-market updates → ONE debounced refetch (2s)
  //   - resync → refetch now
  const [pendingNew, setPendingNew] = useState(0)
  const knownMintsRef = useRef<Set<string>>(new Set())
  const tickDebounce = useRef<ReturnType<typeof setTimeout> | null>(null)
  useTorchFeed('all', (frame) => {
    if (frame.kind === 'resync') {
      fetchTokensRef.current?.(true)
      return
    }
    const mint = typeof frame.mint === 'string' ? frame.mint : undefined
    if (frame.kind === 'market' && mint && !knownMintsRef.current.has(mint)) {
      knownMintsRef.current.add(mint)
      setPendingNew((n) => n + 1)
      return
    }
    if (frame.kind === 'trade' || frame.kind === 'swap' || frame.kind === 'market') {
      if (tickDebounce.current) clearTimeout(tickDebounce.current)
      tickDebounce.current = setTimeout(() => fetchTokensRef.current?.(true), 2000)
    }
  })
  // Pin click: pull the new markets in and clear the counter.
  const acceptNewMarkets = useCallback(() => {
    setPendingNew(0)
    fetchTokensRef.current?.(true)
  }, [])
  const fetchTokensRef = useRef<((r: boolean) => void) | null>(null)
  const [currentSlot, setCurrentSlot] = useState<bigint>(BigInt(0))
  const initialLoadDone = useRef(false)

  // Persistent enrichment data — survives subscription refreshes
  const enrichmentMapRef = useRef(new Map<string, TokenEnrichment>())
  const [enrichmentVersion, setEnrichmentVersion] = useState(0)

  // Merge raw tokens with enrichment data
  const tokens = useMemo(() => {
    const map = enrichmentMapRef.current
    if (map.size === 0) return rawTokens
    return rawTokens.map((t) => {
      const e = map.get(t.mint)
      return e ? { ...t, ...e } : t
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rawTokens, enrichmentVersion])

  const fetchTokens = useCallback(
    async (isRefresh = false) => {
      try {
        const result = await getTokens(connection, {}, { indexer: effectiveIndexerUrl })

        // Smart update: only replace if data actually changed
        setRawTokens((prev) => {
          if (prev.length !== result.tokens.length) return result.tokens
          const changed = result.tokens.some((newToken) => {
            const oldToken = prev.find((t) => t.mint === newToken.mint)
            if (!oldToken) return true
            return (
              oldToken.price_sol !== newToken.price_sol ||
              oldToken.status !== newToken.status ||
              oldToken.progress_percent !== newToken.progress_percent
            )
          })
          return changed ? result.tokens : prev
        })
      } catch (error) {
        console.error('Error fetching tokens:', error)
      } finally {
        if (!isRefresh) {
          setLoading(false)
          initialLoadDone.current = true
        }
      }
    },
    [connection, effectiveIndexerUrl],
  )

  useEffect(() => {
    fetchTokensRef.current = fetchTokens
  }, [fetchTokens])

  useEffect(() => {
    for (const t of tokens) knownMintsRef.current.add(t.mint)
  }, [tokens])

  useEffect(() => {
    if (!enabled) {
      setLoading(false)
      return
    }

    fetchTokens(false)

    // Fetch current slot for relative time display
    connection.getSlot().then((s) => setCurrentSlot(BigInt(s))).catch(() => {})

    const programId = isSimnet ? SIMNET_PROGRAM_ID : PROGRAM_ID

    // Subscribe to program account changes for live updates
    // Skip on simnet - surfpool doesn't support programSubscribe
    if (isSimnet) {
      // Poll instead on simnet (with isRefresh=true to avoid loading flicker)
      // Poll is the FALLBACK: slow heartbeat while the live feed is open,
      // full rate when it isn't (prompt-003 rooms; RPC path unchanged).
      const pollInterval = setInterval(() => fetchTokens(true), feedHealthy ? 60000 : 10000)
      return () => clearInterval(pollInterval)
    }

    // On change, decode the updated BondingCurve directly from the
    // subscription event and patch the enrichment map. This keeps the
    // per-card progress bar moving in real time as trades land, without
    // waiting for the indexer to commit + a full SDK refetch round-trip.
    // We ALSO trigger an SDK refetch in the background so any newly
    // appeared mints (e.g., create_token) show up.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const subCoder = new BorshCoder(idl as any)
    const subscriptionId = connection.onProgramAccountChange(
      programId,
      (keyedAccountInfo) => {
        try {
          const bc = subCoder.accounts.decode(
            'BondingCurve',
            keyedAccountInfo.accountInfo.data,
          ) as BondingCurve
          const mint = bc.mint.toString()
          const map = enrichmentMapRef.current
          const existing = map.get(mint)
          const realSol = BigInt(bc.real_sol_reserves.toString())
          const targetBigInt = BigInt(bc.bonding_target.toString())
          const newProgress = bc.migrated
            ? 100
            : calculateBondingProgress(realSol, targetBigInt)
          const newActivity = Number(bc.last_activity_slot.toString())
          if (existing) {
            existing.progress_percent = newProgress
            existing.last_activity_at = newActivity
          } else {
            // Mint not yet in the enrichment map (e.g., just created).
            // Seed a minimal entry; the regular enrichment useEffect will
            // backfill image/tier/creator on next pass via fetchTokens.
            map.set(mint, {
              progress_percent: newProgress,
              last_activity_at: newActivity,
              bonding_target: Number(bc.bonding_target.toString()),
              creator: bc.creator.toString(),
              tier: getTierFromTarget(Number(bc.bonding_target.toString())),
            })
          }
          setEnrichmentVersion((v) => v + 1)
        } catch {
          // Account isn't a BondingCurve — ignore (filter should prevent
          // this, but defensive).
        }
        fetchTokens(true)
      },
      {
        commitment: 'confirmed',
        filters: [{ memcmp: { offset: 0, bytes: '4y6pru6YvC7' } }],
      },
    )

    return () => {
      connection.removeProgramAccountChangeListener(subscriptionId)
    }
  }, [connection, isSimnet, enabled, fetchTokens])

  // Enrich tokens with image/creator data via direct on-chain reads.
  // Uses getMultipleAccountsInfo + metadata URI fetch instead of getToken()
  // to avoid CoinGecko CORS errors and unnecessary holders calls.
  useEffect(() => {
    if (rawTokens.length === 0 || loading) return
    let cancelled = false

    async function enrichTokens() {
      const map = enrichmentMapRef.current
      const unenriched = rawTokens.filter((t) => !map.has(t.mint))
      if (unenriched.length === 0) return

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const coder = new BorshCoder(idl as any)

      // Batch fetch bonding curve PDAs (100 at a time via getMultipleAccountsInfo)
      const BATCH_SIZE = 100
      for (let i = 0; i < unenriched.length; i += BATCH_SIZE) {
        if (cancelled) return
        const batch = unenriched.slice(i, i + BATCH_SIZE)

        // Derive all bonding curve PDAs
        const pdas = batch.map((t) => getBondingCurvePda(new PublicKey(t.mint))[0])

        let accounts: (import('@solana/web3.js').AccountInfo<Buffer> | null)[]
        try {
          accounts = await connection.getMultipleAccountsInfo(pdas)
        } catch {
          continue
        }
        if (cancelled) return

        // Decode bonding curves and prepare to batch-fetch Token-2022 metadata.
        const metaFetches: Promise<void>[] = []
        // Track migrated tokens in this batch for pool price correction
        const migratedInBatch: { mint: string; bc: BondingCurve; enrichment: TokenEnrichment }[] = []
        // Collect enrichment refs by mint so the URI fetch below can write back.
        const enrichmentsForBatch: { mint: string; enrichment: TokenEnrichment }[] = []

        for (let j = 0; j < batch.length; j++) {
          const acc = accounts[j]
          if (!acc) continue

          let bc: BondingCurve
          try {
            bc = coder.accounts.decode('BondingCurve', acc.data) as BondingCurve
          } catch {
            continue
          }

          const mint = batch[j].mint
          const bondingTargetLamports = Number(bc.bonding_target.toString())
          const tier = getTierFromTarget(bondingTargetLamports)
          const realSolReserves = BigInt(bc.real_sol_reserves.toString())
          const bondingTargetBigInt = BigInt(bc.bonding_target.toString())

          const enrichment: TokenEnrichment = {
            creator: bc.creator.toString(),
            last_activity_at: Number(bc.last_activity_slot.toString()),
            bonding_target: bondingTargetLamports,
            tier,
            // Migrated tokens are always 100%; otherwise correct for actual bonding target
            progress_percent: bc.migrated ? 100 : calculateBondingProgress(realSolReserves, bondingTargetBigInt),
          }

          // Indexer-supplied image: pre-populate so the slow arweave HTTPS
          // fetch below short-circuits. Live BondingCurve fields above are
          // still authoritative — only the visual asset comes from the
          // indexer's metadata cache.
          if (batch[j].image) {
            enrichment.image = batch[j].image
          }

          if (bc.migrated) {
            migratedInBatch.push({ mint, bc, enrichment })
          }

          enrichmentsForBatch.push({ mint, enrichment })

          // Store enrichment (image may be added async, pool price added below)
          map.set(mint, enrichment)
        }

        // [v20] Token-2022 metadata extension holds name/symbol/uri. Batch
        // fetch the URIs for image enrichment.
        if (enrichmentsForBatch.length > 0) {
          metaFetches.push(
            (async () => {
              try {
                const mintPks = enrichmentsForBatch.map((e) => new PublicKey(e.mint))
                const metaMap = await fetchMintsMetadata(connection, mintPks)
                await Promise.allSettled(
                  enrichmentsForBatch.map(async ({ mint, enrichment }) => {
                    // Skip if the indexer already gave us an image — saves
                    // a per-mint arweave HTTPS fetch (the slowest step).
                    if (enrichment.image) return
                    const md = metaMap.get(mint)
                    const uri = md?.uri
                    if (!uri || !isLikelyValidMetadataUri(uri)) return
                    // Irys gateway is dead — rewrite to arweave.net
                    const fetchUrl = uri.includes('gateway.irys.xyz')
                      ? uri.replace('gateway.irys.xyz', 'arweave.net')
                      : uri.includes('uploader.irys.xyz')
                        ? uri.replace('uploader.irys.xyz', 'arweave.net')
                        : uri
                    try {
                      const res = await fetch(fetchUrl)
                      if (!res.ok) return
                      const data = await res.json()
                      if (data.image) {
                        enrichment.image = data.image.includes('gateway.irys.xyz')
                          ? data.image.replace('gateway.irys.xyz', 'arweave.net')
                          : data.image.includes('uploader.irys.xyz')
                            ? data.image.replace('uploader.irys.xyz', 'arweave.net')
                            : data.image
                      }
                    } catch { /* per-mint URI fetch failed */ }
                  }),
                )
              } catch { /* metadata batch failed */ }
            })(),
          )
        }

        // Fetch DeepPool reserves for migrated tokens to get live market cap.
        // DeepPool stores SOL on the pool PDA's lamports and tokens in a Token-2022 vault.
        if (migratedInBatch.length > 0) {
          try {
            const accountPdas: PublicKey[] = []
            const vaultMeta: { mint: string; bc: BondingCurve; enrichment: TokenEnrichment }[] = []
            for (const entry of migratedInBatch) {
              const { pool, tokenVault } = getDeepPoolAccounts(new PublicKey(entry.mint))
              accountPdas.push(pool, tokenVault)
              vaultMeta.push(entry)
            }

            const poolAccounts = await connection.getMultipleAccountsInfo(accountPdas)

            // Rent-exempt depends on the pool account's data length. We cache by
            // data length to avoid one RPC per token in a hot batch.
            const rentByLen = new Map<number, number>()

            for (let k = 0; k < vaultMeta.length; k++) {
              const poolAcc = poolAccounts[k * 2]
              const tokenVaultAcc = poolAccounts[k * 2 + 1]
              const { enrichment } = vaultMeta[k]

              if (!poolAcc) continue
              let rent = rentByLen.get(poolAcc.data.length)
              if (rent === undefined) {
                rent = await connection.getMinimumBalanceForRentExemption(poolAcc.data.length)
                rentByLen.set(poolAcc.data.length, rent)
              }
              const sol = BigInt(poolAcc.lamports) - BigInt(rent)
              const poolSolBalance = sol > BigInt(0) ? sol : BigInt(0)

              const poolTokenBalance = tokenVaultAcc && tokenVaultAcc.data.length >= 72
                ? tokenVaultAcc.data.readBigUInt64LE(64) : BigInt(0)

              if (poolTokenBalance > BigInt(0) && poolSolBalance > BigInt(0)) {
                const price = Number(poolSolBalance) / Number(poolTokenBalance)
                const priceInSol = (price * TOKEN_MULTIPLIER) / LAMPORTS_PER_SOL
                enrichment.price_sol = priceInSol
                // Market cap = fully diluted (total supply × price)
                enrichment.market_cap_sol = (priceInSol * Number(TOTAL_SUPPLY)) / TOKEN_MULTIPLIER
              }
            }
          } catch {
            // DeepPool fetch failed — keep SDK market cap as fallback
          }
        }

        // Wait for all metadata fetches in this batch
        if (metaFetches.length > 0) {
          await Promise.allSettled(metaFetches)
        }

        if (cancelled) return
        setEnrichmentVersion((v) => v + 1)
      }
    }

    enrichTokens()
    return () => { cancelled = true }
  }, [rawTokens.length, loading, connection])

  // Compute filter counts
  const filterCounts = useMemo(
    () => ({
      bonding: tokens.filter((t) => matchesFilter(t, 'bonding')).length,
      complete: tokens.filter((t) => matchesFilter(t, 'complete')).length,
      reclaimed: tokens.filter((t) => matchesFilter(t, 'reclaimed')).length,
    }),
    [tokens],
  )

  // Top Projects: highest market cap completed/migrated tokens
  const topProjects = useMemo(
    () =>
      tokens
        .filter((t) => t.status === 'complete' || t.status === 'migrated')
        .sort((a, b) => b.market_cap_sol - a.market_cap_sol)
        .slice(0, 10),
    [tokens],
  )

  // Trending: tokens between 75-100% progress (still bonding)
  const trendingTokens = useMemo(
    () =>
      tokens
        .filter((t) => t.status === 'bonding' && t.progress_percent >= 75 && t.progress_percent < 100)
        .sort((a, b) => b.progress_percent - a.progress_percent),
    [tokens],
  )

  // Group tokens by tier
  const tokensByTier = useMemo(() => {
    const groups: Record<TokenTier, TokenData[]> = { flame: [], torch: [] }
    for (const t of tokens) {
      const tier = t.tier || 'torch' // Default to torch for pre-v3.3.0 tokens
      groups[tier].push(t)
    }
    return groups
  }, [tokens])

  const tierCounts = useMemo(
    () => ({
      flame: tokensByTier.flame.length,
      torch: tokensByTier.torch.length,
    }),
    [tokensByTier],
  )

  return {
    tokens,
    loading,
    pendingNew,
    acceptNewMarkets,
    filterCounts,
    topProjects,
    trendingTokens,
    currentSlot,
    tokensByTier,
    tierCounts,
  }
}

/**
 * Filter and sort tokens based on filter and search
 */
export function filterAndSortTokens(
  tokens: TokenData[],
  filter: TokenFilter,
  search: string,
): TokenData[] {
  const hasSearch = search.trim().length > 0

  return tokens
    .filter((t) => {
      // When searching, match across all tokens regardless of status filter
      if (hasSearch) {
        const searchLower = search.toLowerCase().trim()
        const matchesMint = t.mint.toLowerCase().includes(searchLower)
        const matchesName = t.name.toLowerCase().includes(searchLower)
        const matchesSymbol = t.symbol.toLowerCase().includes(searchLower)
        return matchesMint || matchesName || matchesSymbol
      }
      // No search — filter by status
      return matchesFilter(t, filter)
    })
    .sort((a, b) => {
      // Bonding tokens first, then by created_at (most recent first)
      const aIsBonding = a.status === 'bonding'
      const bIsBonding = b.status === 'bonding'

      if (aIsBonding && !bIsBonding) return -1
      if (!aIsBonding && bIsBonding) return 1

      // Within same category, sort by created_at (most recent first)
      return b.created_at - a.created_at
    })
}
