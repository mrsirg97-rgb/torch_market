pub const TOTAL_SUPPLY: u64 = 1_000_000_000_000_000;
pub const MAX_WALLET_TOKENS: u64 = 20_000_000_000_000;
pub const TREASURY_SOL_MAX_BPS: u16 = 1750; // 17.5% at start
pub const TREASURY_SOL_MIN_BPS: u16 = 250; // 2.5% at completion
pub const TREASURY_FEE_BPS: u16 = 0;
pub const DEV_WALLET_SHARE_BPS: u16 = 5000; // 50% of protocol fee to dev, 50% to user rewards
pub const SELL_FEE_BPS: u16 = 0;
pub const BONDING_TARGET_LAMPORTS: u64 = 200_000_000_000;
pub const BONDING_TARGET_SPARK: u64 = 50_000_000_000; // 50 SOL
pub const BONDING_TARGET_FLAME: u64 = 100_000_000_000; // 100 SOL
pub const BONDING_TARGET_TORCH: u64 = 200_000_000_000; // 200 SOL (default)
pub const VALID_BONDING_TARGETS: [u64; 2] = [BONDING_TARGET_FLAME, BONDING_TARGET_TORCH];
pub const TOKEN_DECIMALS: u8 = 6;
pub const INITIAL_VIRTUAL_SOL: u64 = 30_000_000_000;
pub const INITIAL_VIRTUAL_TOKENS: u64 = 107_300_000_000_000;
pub const TREASURY_LOCK_TOKENS: u64 = 300_000_000_000_000;
pub const CURVE_SUPPLY: u64 = 700_000_000_000_000;
pub const TREASURY_LOCK_SEED: &[u8] = b"treasury_lock";
pub const INITIAL_VIRTUAL_TOKENS_V27: u64 = 756_250_000_000_000;

pub fn initial_virtual_reserves(bonding_target: u64) -> (u64, u64) {
    match bonding_target {
        BONDING_TARGET_SPARK => (18_750_000_000, INITIAL_VIRTUAL_TOKENS_V27), // 18.75 SOL
        BONDING_TARGET_FLAME => (37_500_000_000, INITIAL_VIRTUAL_TOKENS_V27), // 37.5 SOL
        BONDING_TARGET_TORCH => (75_000_000_000, INITIAL_VIRTUAL_TOKENS_V27), // 75 SOL
        _ => (INITIAL_VIRTUAL_SOL, INITIAL_VIRTUAL_TOKENS),                   // Legacy
    }
}

pub const PROTOCOL_FEE_BPS: u16 = 50;
pub const MIN_SOL_AMOUNT: u64 = 1_000_000;
pub const TRANSFER_FEE_BPS: u16 = 7;
pub const MAX_TRANSFER_FEE: u64 = u64::MAX; // Uncapped per Token-2022 spec; actual fee governed by TRANSFER_FEE_BPS
pub const GLOBAL_CONFIG_SEED: &[u8] = b"global_config";
pub const BONDING_CURVE_SEED: &[u8] = b"bonding_curve";
pub const TREASURY_SEED: &[u8] = b"treasury";
// System-owned PDA that physically custodies the per-token treasury's SOL.
// The `Treasury` account holds accounting/TWAP state (program-owned, so torch can
// write it); the lendable lamports live here (System-owned, so they move via
// `system_program::transfer`). See project_v21_long_sol_flow.
pub const TREASURY_SOL_VAULT_SEED: &[u8] = b"treasury_sol_vault";
pub const USER_POSITION_SEED: &[u8] = b"user_position";
pub const USER_STATS_SEED: &[u8] = b"user_stats";
pub const INACTIVITY_PERIOD_SLOTS: u64 = 7 * 24 * 60 * 60 * 1000 / 400;
pub const EPOCH_DURATION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const MIN_RECLAIM_THRESHOLD: u64 = 10_000_000;
pub const MIGRATION_SEED: &[u8] = b"migration";
// [V21] System-owned SOL custody for the bonding curve. The curve DATA account
// keeps the projected `real_sol_reserves` (the donation-immune bonding gate), but
// the actual bonded lamports live here — so buy/sell/reclaim/migration move SOL via
// seed-signed system transfers (zero direct-lamport on the curve) and migration
// seed-signs this PDA as deep_pool create_pool's sol_source (curve → pool, never a
// user wallet). Created lazily on first buy. Mirrors treasury_sol_vault / vault_sol.
pub const BONDING_CURVE_SOL_SEED: &[u8] = b"bonding_curve_sol";
pub const MIN_MIGRATION_SOL: u64 = 1_500_000_000; // 1.5 SOL
                                                  // DeepPool Program ID: CcwF61GW14AcxCS4E2zedHXdFXy8x8GQPvfxZrs2x2eT
pub const DEEP_POOL_PROGRAM_ID: anchor_lang::prelude::Pubkey = deep_pool::ID;
pub const DEEP_POOL_POOL_SEED: &[u8] = b"deep_pool";
pub const DEEP_POOL_VAULT_SEED: &[u8] = b"pool_vault";
pub const DEEP_POOL_LP_MINT_SEED: &[u8] = b"pool_lp_mint";
pub const TORCH_CONFIG_SEED: &[u8] = b"torch_config";
pub const CREATOR_FEE_SHARE_BPS: u16 = 1500;
pub const CREATOR_SOL_MIN_BPS: u16 = 20;
pub const CREATOR_SOL_MAX_BPS: u16 = 100;

// `compute_buy_split` computes `sol_to_treasury_split = total_split - creator_sol`
// via checked_sub. Both rates are linear and clamped in [MIN, MAX]; the subtraction
// is underflow-free iff `creator_rate(x) <= treasury_rate(x)` at both endpoints.
const _: () = assert!(CREATOR_SOL_MIN_BPS <= TREASURY_SOL_MAX_BPS);
const _: () = assert!(CREATOR_SOL_MAX_BPS <= TREASURY_SOL_MIN_BPS);
pub const DEX_BUYBACK_MIN_SLIPPAGE_BPS: u16 = 100;
pub const DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS: u64 = 2700;
pub const RATIO_PRECISION: u128 = 1_000_000_000;
pub const DEFAULT_SELL_THRESHOLD_BPS: u16 = 12000;
pub const DEFAULT_SELL_PERCENT_BPS: u16 = 1500;
pub const SELL_ALL_TOKEN_THRESHOLD: u64 = 1_000_000_000_000;
pub const PROTOCOL_TREASURY_SEED: &[u8] = b"protocol_treasury_v11";
pub const PROTOCOL_TREASURY_RESERVE_FLOOR: u64 = 0;
pub const MIN_EPOCH_VOLUME_ELIGIBILITY: u64 = 2_000_000_000;
pub const MIN_CLAIM_AMOUNT: u64 = 100_000_000;
pub const MAX_CLAIM_SHARE_BPS: u64 = 1_000;
pub const REVIVAL_THRESHOLD: u64 = INITIAL_VIRTUAL_SOL;
pub const COLLATERAL_VAULT_SEED: &[u8] = b"collateral_vault";
pub const LOAN_SEED: &[u8] = b"loan";
// [V21] D-7 lowers this to 150 (1.5% APR) to pair with OPEN_FEE_BPS for a
// ~neutral medium-term cost-of-leverage. Seeded into treasury.interest_rate_bps
// at init (token.rs); the sim (torch_sim.py) tracks the same 150 bps.
pub const DEFAULT_INTEREST_RATE_BPS: u16 = 150;
// [V21] Flat open fee on the SOL-denominated borrow value, routed to treasury
// at open (D-4/D-9). SOL leg regardless of side: collateral for shorts, borrow
// for longs. Leverage-proportional — a 50% LTV position pays half a 100% one,
// so conservative borrowers don't subsidize aggressive ones. Consumed by the
// V21 open handlers (step 4) via apply_bps; inert until then.
pub const OPEN_FEE_BPS: u16 = 50;
// [V21] Ceiling on the depth-scaled max-LTV curve (== LTV_MAX_BPS asymptote, so
// it never binds below the asymptote). Seeded into treasury.max_ltv_bps at init.
pub const DEFAULT_MAX_LTV_BPS: u16 = 6000;
pub const DEFAULT_LIQUIDATION_THRESHOLD_BPS: u16 = 6500;
// [V21] Liquidation bonus CEILING, derived = 1.3·ρ_max (BONUS_SAFETY_BPS over the
// ρ_max size-cap slippage). At RHO_MAX_BPS=2500 → 3250 bps (32.5%). Seeded into
// treasury.liquidation_bonus_bps; the realized bonus ramps to this on the TWAP
// LTV (effective_liq_bonus_bps). See docs/depth-scaled-risk-rails.md (Rail 3).
pub const DEFAULT_LIQUIDATION_BONUS_BPS: u16 =
    (RHO_MAX_BPS as u32 * BONUS_SAFETY_BPS as u32 / 10_000) as u16;
pub const DEFAULT_LIQUIDATION_CLOSE_BPS: u16 = 5000;
pub const DEFAULT_LENDING_UTILIZATION_CAP_BPS: u16 = 8000;
pub const MIN_BORROW_AMOUNT: u64 = 100_000_000;
pub const BORROW_SHARE_MULTIPLIER: u64 = 23; // Per-user cap: max borrow = lendable * (collateral / denominator) * multiplier
// Hard ceiling on per-user borrow regardless of collateral size. Without
// this, the BORROW_SHARE_MULTIPLIER allows a user with >~4.35% of total
// supply (= TOTAL_SUPPLY / multiplier) as collateral to take the entire
// lendable amount — defeating the per-user cap entirely. 2000 bps = 20%
// ceiling: each whale gets a meaningful 1/5 slice of lendable, leaving
// room for at least 5 simultaneous concentrated positions. By design:
// smaller per-user positions → more positions in flight → more
// liquidation opportunities → more hunters/shorters participating in
// the game.
pub const MAX_USER_BORROW_SHARE_BPS: u16 = 2000;
pub const EPOCH_DURATION_SLOTS: u64 = 7 * 24 * 60 * 60 * 1000 / 400; // ~7 days at 400ms/slot
pub const METADATA_POINTER_EXTENSION_SIZE: usize = 68;
pub const TOKEN_METADATA_FIXED_SIZE: usize = 80;
pub const EXTENSION_TLV_HEADER_SIZE: usize = 4;
pub const TORCH_VAULT_SEED: &[u8] = b"torch_vault";
// System-owned companion PDA holding SOL for the duration of a deep_pool swap
// CPI. Only used in vault_swap; sits at 0 lamports between swaps.
pub const TORCH_VAULT_SOL_SEED: &[u8] = b"torch_vault_sol";
pub const VAULT_WALLET_LINK_SEED: &[u8] = b"vault_wallet";
pub const SHORT_SEED: &[u8] = b"short";
pub const SHORT_CONFIG_SEED: &[u8] = b"short_config";
/// Prevents dust positions that cost more in rent than they're worth
pub const MIN_SHORT_TOKENS: u64 = 1_000_000_000;

// [V21] Per-token closed leverage — unified Position + per-position vaults.
// The Position PDA is `[POSITION_SEED, user, mint, [side], index_le]`; the side
// byte (POSITION_SIDE_*) disambiguates a long vs a short at the same index.
pub const POSITION_SEED: &[u8] = b"position";
// Per-position SOL vault, system-owned (0-data PDA holding lamports). Shorts:
// persistent collateral + sale proceeds. Longs: transient SOL stage for the
// deep_pool swap legs (treasury isn't system-owned, so it can't be the swap's
// `sol_source` directly). Seed: `[<seed>, user, mint, index_le]`.
pub const SHORT_VAULT_SEED: &[u8] = b"short_vault";
pub const LONG_SOL_VAULT_SEED: &[u8] = b"long_sol_vault";
// Side bytes for the Position PDA seed. MUST mirror PositionSide's repr(u8)
// discriminants (state.rs) — enforced by repr(u8) + explicit values there.
pub const POSITION_SIDE_LONG: u8 = 0;
pub const POSITION_SIDE_SHORT: u8 = 1;
pub const MIN_POOL_SOL_LENDING: u64 = 5_000_000_000;

// Lending unlocks once the protocol has accumulated enough SOL fees in the
// treasury to make borrowing meaningful. Threshold is volume-driven, NOT
// price-driven — it grows from trade fees + 4× transfer fees per short
// cycle + interest, so it gates lending on actual protocol activity rather
// than oracle-pumpable price signals.
//
// Build flag per environment (see Cargo.toml `[features]`):
//   `simnet`  → 1 SOL (migration seeds the treasury well above this — lending
//               unlocks naturally after bonding, no volume simulation needed)
//   `devnet`  → 1 SOL (achievable with light e2e activity)
//   default   → 100 SOL (mainnet; the real bar — see docs/lending-unlock.md)
//
// simnet and devnet share the same 1 SOL gate, so a position posted below it
// (e.g. 0.5 SOL) is rejected under EVERY build — the lock path is testable
// build-independently. Belt-and-suspenders: both flags = compile_error below.
#[cfg(any(feature = "simnet", feature = "devnet"))]
pub const MIN_TREASURY_SOL_FOR_LENDING: u64 = 1_000_000_000; // 1 SOL (simnet/devnet)

#[cfg(not(any(feature = "simnet", feature = "devnet")))]
pub const MIN_TREASURY_SOL_FOR_LENDING: u64 = 100_000_000_000; // 100 SOL (mainnet)

#[cfg(all(feature = "simnet", feature = "devnet"))]
compile_error!("only one of `simnet` or `devnet` features may be enabled at a time");
// [V21] Depth-scaled risk rails (continuous + concave). Replaces the old 4-step
// LTV ladder. Depth is priced ONCE at open via two pure functions in
// pool_validation (get_depth_max_ltv_bps + max_debt_value_for_depth); the
// liquidation logic itself is unchanged. See docs/depth-scaled-risk-rails.md.
//
// Rail 1 — max-LTV curve: LTV(S) = LTV_MAX − (LTV_MAX−LTV_MIN)·(S_floor/S),
// anchored at the smallest pool we lever (S_floor). Concave (diminishing
// safety-returns to depth) and division-only (Kani-friendly, α=1). Below the
// floor → 0 (no leverage).
pub const DEPTH_FLOOR_SOL: u64 = 100_000_000_000; // 100 SOL — smallest pool we lever
pub const LTV_MIN_BPS: u16 = 3000; // 30% at the floor
pub const LTV_MAX_BPS: u16 = 6000; // 60% asymptote (== DEFAULT_MAX_LTV_BPS ceiling)
// Rail 2 — size cap: a position's SOL-debt-value ≤ ρ_max of pool SOL, so the
// worst-case unwind slippage (≈ debt/pool_sol) is depth-invariant and ONE flat
// bonus clears it on every pool. Clamped at open (UI shows the clamped size).
pub const RHO_MAX_BPS: u16 = 2500; // 25%
// Rail 3 — bonus ceiling = BONUS_SAFETY_BPS·ρ_max (margin over the ρ_max
// slippage). Drives DEFAULT_LIQUIDATION_BONUS_BPS + LIQ_FULL_BONUS_LTV_BPS.
pub const BONUS_SAFETY_BPS: u16 = 13000; // 1.3×

// [V21][audit V21-2/V21-3] Fail-closed invariants on the depth-rail constants.
// The curve's `span = LTV_MAX − LTV_MIN` (u16) must not underflow, and the size
// cap `ρ_max·pool/10000` must keep `ρ_max < 10000` so `max_debt_value_for_depth`
// can never truncate `as u64` above pool_sol. A future mis-edit fails the build.
const _: () = assert!(LTV_MAX_BPS > LTV_MIN_BPS);
const _: () = assert!(RHO_MAX_BPS < 10_000);

// [V21][D-10] Hardened TWAP liquidation mark — manipulation-resistant.
//
// The liquidation TRIGGER (LTV) and SEIZE accounting mark against a
// time-weighted average price, never raw spot — closing the spot-AMM-as-oracle
// hole (docs/v21-closed-loop-leverage.md §D-10). As of v21 the oracle itself
// lives in DeepPool (keeperless: a CPMM's price moves only on swaps, so the pool
// accumulates on the swap and there is nothing to sample between swaps). Torch is
// a pure CONSUMER — it deserializes the `deep_pool::Pool` already present in the
// liquidation context and reads a Q64.64 time-weighted SOL-per-token price over
// its own lookback. No ring, no crank, no per-observation ratchet live in torch
// anymore (see docs/twap-oracle.md). The pricing math (math::twap_value_in_sol /
// twap_tokens_to_seize) is the only TWAP surface that stays here.
//
// Window length is the CONSUMER's risk policy — torch picks the lookback the
// liquidation mark averages over. Longer = harder to manipulate (an attacker
// must HOLD an off-market price across the whole window, bleeding to arbitrage
// every block) but laggier to liquidate a genuine move; shorter is snappier but
// cheaper to nudge. The realized window is ≥ lookback and ≤ lookback + DeepPool's
// MIN_OBS_SPACING_SLOTS. DeepPool's ring spans ~8000 slots (~53 min @ 400ms), so
// the lookback must stay under that or the read fails closed (warmup).
pub const LIQ_TWAP_LOOKBACK_SLOTS: u64 = 3000; // ~20 min @ 400ms/slot
// Liquidation bonus ramps 0 (at the liq threshold) → full (here), measured on
// the hardened LTV, so a manufactured barely-over liquidation earns ~0 prize.
// [V21] DERIVED so a liquidator clearing a ≤ρ_max unwind is always whole at the
// point insolvency would begin: full_bonus_ltv = 100/(1+bonus). At bonus=3250 →
// 7547 bps (75.5%). Coupled to the ρ_max/safety knobs above (Rail 3).
pub const LIQ_FULL_BONUS_LTV_BPS: u16 =
    (100_000_000u32 / (10_000 + DEFAULT_LIQUIDATION_BONUS_BPS as u32)) as u16;
// [V21] Spot LTV can only VETO a TWAP-triggered liquidation when it is CLEARLY
// healthy — at least this many LTV bps BELOW the liquidation threshold. The TWAP
// is the binding trigger (manipulation-resistant; seize is TWAP-priced); the spot
// check is kept only to refuse liquidating a genuinely-recovered borrower on a
// stale-high TWAP. Setting the veto floor below the threshold removes the cheap
// "nudge spot just under the threshold to dodge a real liquidation" vector — a
// dodge now requires shoving spot a full margin below the line (expensive on a
// thin pool, and a sustained move heals the TWAP anyway).
pub const LIQ_SPOT_VETO_MARGIN_BPS: u64 = 1000; // 10 LTV points below threshold
