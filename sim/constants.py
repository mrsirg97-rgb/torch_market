"""Protocol constants — mirror of programs/torch_market/src/constants.rs
(+ the depth-rail pure functions from pool_validation.rs).
All math mirrors the on-chain Rust (integer arithmetic).
"""
# Extracted from the former monolithic torch_sim.py (split 2026-06-09).
# torch_sim.py remains the entry point / public surface.

# ============================================================================
# Constants (from constants.rs)
# ============================================================================

TOTAL_SUPPLY        = 1_000_000_000_000_000   # 1B tokens (6 decimals)
CURVE_SUPPLY        = 700_000_000_000_000     # 70%
TREASURY_LOCK       = 300_000_000_000_000     # 30%
TOKEN_DECIMALS      = 6
LAMPORTS_PER_SOL    = 1_000_000_000

# Bonding targets
BONDING_TARGET_FLAME = 100_000_000_000        # 100 SOL
BONDING_TARGET_TORCH = 200_000_000_000        # 200 SOL

# Virtual reserves (V27, Torch tier)
INITIAL_VIRTUAL_SOL    = 75_000_000_000       # 75 SOL
INITIAL_VIRTUAL_TOKENS = 756_250_000_000_000

# Fees
PROTOCOL_FEE_BPS       = 50    # 0.5%
TREASURY_SOL_MAX_BPS   = 1750  # 17.5%
TREASURY_SOL_MIN_BPS   = 250   # 2.5%
CREATOR_SOL_MIN_BPS    = 20    # 0.2%
CREATOR_SOL_MAX_BPS    = 100   # 1.0%
DEV_WALLET_SHARE_BPS   = 5000  # 50% of protocol fee → dev wallet, 50% → protocol_treasury
TRANSFER_FEE_BPS       = 7     # 0.07%
CREATOR_FEE_SHARE_BPS  = 1500  # 15% of swap_fees_to_sol proceeds

# Per-wallet hard cap on bonding-curve buys (anti-concentration).
MAX_WALLET_TOKENS      = 20_000_000_000_000   # 20M tokens at 6 decimals

# Short-selling gates.
MIN_SHORT_TOKENS       = 1_000_000_000        # 1k tokens; rejects dust shorts

# Migration cost — approximate rent + LP-mint cost the payer eats during
# migrate_to_dex, reimbursed from treasury. Real value is dynamic (rent rates
# + account sizes); 0.02 SOL is a reasonable midpoint for invariant tests.
MIGRATION_COST_APPROX  = 20_000_000           # 0.02 SOL

# Treasury swap_fees_to_sol gates (mirror constants.rs)
DEFAULT_SELL_THRESHOLD_BPS          = 12000      # 120% of baseline ratio (i.e. price 20% above)
DEFAULT_SELL_PERCENT_BPS            = 1500       # sell 15% of held tokens per harvest
DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS  = 2700       # ~18 min cooldown between harvests
SELL_ALL_TOKEN_THRESHOLD            = 1_000_000_000_000  # below this balance, sell 100%

# Lending
DEFAULT_INTEREST_RATE_BPS = 150   # 1.5% per epoch (V21: 2% → 1.5%, pairs with open fee)
DEFAULT_MAX_LTV_BPS      = 6000  # 60% — ceiling = LTV_MAX_BPS asymptote (see depth curve below)
OPEN_FEE_BPS             = 50    # 0.5% of borrow value in SOL, charged to treasury on open
DEFAULT_LIQ_THRESHOLD    = 6500  # 65%
# [depth-scaled risk rails] The liquidation bonus is DERIVED from the size cap
# ρ_max, not hand-set: with each position's SOL-debt-value ≤ ρ_max·pool_sol (see
# RHO_MAX_BPS + max_debt_value_for_depth), worst-case unwind slippage is ρ_max on
# EVERY pool, so a single flat bonus sized to clear it works everywhere — no
# bonus(S) curve needed. See docs/depth-scaled-risk-rails.md.
RHO_MAX_BPS              = 2500  # size cap: a position's SOL-debt-value ≤ 25% of pool SOL
BONUS_SAFETY_BPS         = 13000 # 1.3× — liquidator margin over the ρ_max slippage
DEFAULT_LIQ_BONUS_BPS    = RHO_MAX_BPS * BONUS_SAFETY_BPS // 10000          # ≈ 3250 (32.5%)
# Ramp completes AT the insolvency line (LTV·(1+bonus)=100%), NOT a fixed 90%:
# a bigger bonus lowers the insolvency LTV, so the ramp must mature before it.
LIQ_FULL_BONUS_LTV_BPS   = 10000 * 10000 // (10000 + DEFAULT_LIQ_BONUS_BPS)  # ≈ 7547 (75.5%)
DEFAULT_LIQ_CLOSE_BPS    = 5000  # 50% close factor
LIQ_SPOT_VETO_MARGIN_BPS = 1000  # spot veto: spot may refuse only when CLEARLY healthy
                                 # (threshold − margin below); TWAP is the binding trigger
BORROW_SHARE_MULTIPLIER  = 23
MAX_USER_BORROW_SHARE_BPS = 2000  # 20% absolute cap per user (mirrors on-chain MAX_USER_BORROW_SHARE_BPS)
MIN_TREASURY_SOL_FOR_LENDING = 100_000_000_000  # 100 SOL — lending disabled below this floor
MIN_BORROW_AMOUNT        = 100_000_000  # 0.1 SOL
MIN_POOL_SOL_LENDING     = 5_000_000_000  # 5 SOL
MAX_PRICE_DEVIATION_BPS  = 5000  # 50%

# [depth-scaled risk rails] Continuous, concave max-LTV vs pool depth — replaces
# the old 4-step ladder. LTV(S) = LTV_max − (LTV_max−LTV_min)·(S_floor/S), anchored
# at the smallest pool we lever (S_floor). α=1 ⇒ division-only (Kani-friendly).
# Concave = diminishing safety-returns to depth. See docs/depth-scaled-risk-rails.md.
DEPTH_FLOOR_SOL = 100_000_000_000  # 100 SOL — smallest pool we offer leverage on
LTV_MIN_BPS     = 3000             # 30% at the floor
LTV_MAX_BPS     = 6000             # 60% asymptote

def get_depth_max_ltv_bps(pool_sol: int) -> int:
    if pool_sol < DEPTH_FLOOR_SOL:
        return 0  # below the smallest pool we lever → no leverage
    span = LTV_MAX_BPS - LTV_MIN_BPS
    ltv = LTV_MAX_BPS - span * DEPTH_FLOOR_SOL // pool_sol
    return min(LTV_MAX_BPS, max(LTV_MIN_BPS, ltv))

def max_debt_value_for_depth(pool_sol: int) -> int:
    """Size cap: a position's SOL-debt-value ≤ ρ_max of pool SOL. Makes worst-case
    unwind slippage (≈ debt/pool_sol) depth-invariant, so the flat bonus clears it
    on every pool. Clamped at open (house pattern: the UI shows the clamped size)."""
    return RHO_MAX_BPS * pool_sol // 10000

# Keeperless TWAP oracle. As of V21 the oracle lives in DeepPool (a CPMM's price
# moves only on swaps, so the pool accumulates on the swap — no crank, no
# ratchet). Torch is a pure consumer reading a Q64.64 SOL-per-token mark over its
# own lookback. Mirror of deep_pool/src/{math,state,constants}.rs + torch's
# LIQ_TWAP_LOOKBACK_SLOTS. See docs/twap-oracle.md.
Q64                     = 1 << 64
MASK_128                = (1 << 128) - 1
TWAP_RING_SIZE          = 16             # deep_pool ring of periodic snapshots
MIN_OBS_SPACING_SLOTS   = 500            # min slots between ring snapshots
MIN_SPOT_RESERVE        = 5_000_000_000  # 5 SOL dust floor — thin pools don't feed the mark
LIQ_TWAP_LOOKBACK_SLOTS = 3000           # torch consumer window (~20 min @ 400ms)

# Time
EPOCH_DURATION_SLOTS = 7 * 24 * 60 * 60 * 1000 // 400  # ~7 days
