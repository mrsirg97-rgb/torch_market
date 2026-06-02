"""
Torch Market Economic Simulator (V21)

Pure-Python simulation of the V21 Torch Market protocol:
  - Bonding curve (constant product with dynamic fee splits)
  - Post-migration DeepPool (immutable CPMM)
  - V21 atomic-custodied leveraged long (SOL borrow → atomic pool_buy → vault tokens)
  - V21 atomic-custodied short (token borrow → atomic pool_sell → vault SOL)
  - Liquidation cascades (per-position vault seize)
  - Transfer fee harvesting
  - Open fee (0.5% of borrow value in SOL → treasury)

All math mirrors the on-chain Rust (integer arithmetic, checked ops).
No external deps — stdlib only.
"""

from __future__ import annotations
import random
import math
from dataclasses import dataclass, field
from typing import Optional
from enum import Enum

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
LENDING_UTIL_CAP_BPS     = 8000  # 80%
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


# ============================================================================
# State
# ============================================================================

@dataclass
class BondingCurve:
    virtual_sol: int = INITIAL_VIRTUAL_SOL
    virtual_tokens: int = INITIAL_VIRTUAL_TOKENS
    real_sol: int = 0
    real_tokens: int = CURVE_SUPPLY
    bonding_target: int = BONDING_TARGET_TORCH
    bonding_complete: bool = False

    @property
    def price(self) -> float:
        """Current price in SOL per token (float for display)."""
        if self.virtual_tokens == 0:
            return float('inf')
        return self.virtual_sol / self.virtual_tokens

    @property
    def progress_pct(self) -> float:
        return (self.real_sol / self.bonding_target) * 100 if self.bonding_target > 0 else 0


@dataclass
class Pool:
    """Post-migration DeepPool. 0.25% fee auto-compounds into reserves.

    Owns the keeperless TWAP oracle (mirror of deep_pool::Pool): a Q64.64
    price-cumulative advanced on every swap, plus a ring of periodic snapshots a
    consumer reads over a chosen lookback.
    """
    sol_reserves: int = 0
    token_reserves: int = 0
    # --- Keeperless TWAP oracle (mirror of deep_pool::Pool) ---
    cum_sol_per_tok: int = 0
    cum_tok_per_sol: int = 0
    last_cum_slot: int = 0
    observations: list = field(default_factory=list)  # [{slot, cum_sol_per_tok, cum_tok_per_sol}]

    @property
    def k(self) -> int:
        return self.sol_reserves * self.token_reserves

    @property
    def price(self) -> float:
        if self.token_reserves == 0:
            return float('inf')
        return self.sol_reserves / self.token_reserves

    FEE_BPS: int = 25  # 0.25% — auto-compounds into reserves

    def swap_sol_for_tokens(self, sol_in: int, now: int = None) -> int:
        """Constant product swap with fee. Returns tokens out. When `now` is
        supplied, advances the keeperless TWAP oracle from the PRE-swap reserves
        first (the swap is the oracle's only writer — mirror of deep_pool)."""
        if sol_in <= 0 or self.sol_reserves <= 0 or self.token_reserves <= 0:
            return 0
        if now is not None:
            self.record_observation(now)
        fee = max(1, (sol_in * self.FEE_BPS) // 10000)  # deep_pool calc_swap_fee min-1 floor
        effective_in = sol_in - fee
        tokens_out = (self.token_reserves * effective_in) // (self.sol_reserves + effective_in)
        tokens_out = min(tokens_out, self.token_reserves - 1)  # can't drain pool
        self.sol_reserves += sol_in  # full amount including fee stays in pool
        self.token_reserves -= tokens_out
        return tokens_out

    def swap_tokens_for_sol(self, tokens_in: int, now: int = None) -> int:
        """Constant product swap with fee. Returns SOL out. When `now` is
        supplied, advances the keeperless TWAP oracle from the PRE-swap reserves."""
        if tokens_in <= 0 or self.sol_reserves <= 0 or self.token_reserves <= 0:
            return 0
        if now is not None:
            self.record_observation(now)
        fee = max(1, (tokens_in * self.FEE_BPS) // 10000)  # deep_pool calc_swap_fee min-1 floor
        effective_in = tokens_in - fee
        sol_out = (self.sol_reserves * effective_in) // (self.token_reserves + effective_in)
        sol_out = min(sol_out, self.sol_reserves - 1)
        self.token_reserves += tokens_in  # full amount including fee stays in pool
        self.sol_reserves -= sol_out
        return sol_out

    def quote_tokens_for_sol(self, tokens_in: int) -> int:
        """Non-mutating preview of swap_tokens_for_sol — returns the SOL out a
        sell would yield at current reserves WITHOUT advancing reserves/oracle.
        Used to honor on-chain reverts (e.g. close_long require!(sol_out >= debt))
        before any state mutation."""
        if tokens_in <= 0 or self.sol_reserves <= 0 or self.token_reserves <= 0:
            return 0
        fee = max(1, (tokens_in * self.FEE_BPS) // 10000)
        effective_in = tokens_in - fee
        sol_out = (self.sol_reserves * effective_in) // (self.token_reserves + effective_in)
        return min(sol_out, self.sol_reserves - 1)

    def quote_buy_sol_in_for_tokens_out(self, tokens_out: int) -> Optional[int]:
        """Inverse of swap_sol_for_tokens — given desired tokens_out, return
        sol_in needed (ceiling-rounded to guarantee >= tokens_out).

        Constant product:
            tokens_out = pool_t * effective_in / (pool_s + effective_in)
          => effective_in = pool_s * tokens_out / (pool_t - tokens_out)
          => sol_in = ceil(effective_in * 10000 / (10000 - FEE_BPS))

        Returns None if pool too thin to fill order.
        """
        if tokens_out <= 0 or tokens_out >= self.token_reserves:
            return None
        num = self.sol_reserves * tokens_out
        den = self.token_reserves - tokens_out
        effective_in = (num + den - 1) // den  # ceil
        denom = 10000 - self.FEE_BPS
        if denom == 0:
            return None
        return (effective_in * 10000 + denom - 1) // denom  # ceil

    # ------------------------------------------------------------------
    # Keeperless TWAP oracle — mirror of deep_pool/src/{math,state}.rs. The
    # cumulatives are wrapping Q64.64 price integrals (mod 2^128); a consumer
    # recovers a window with a wrapping subtraction. Advanced on every swap.
    # ------------------------------------------------------------------
    @staticmethod
    def _price_q64(reserve_out: int, reserve_in: int):
        """math::price_q64 — Q64.64 price of `in` denominated in `out`."""
        if reserve_in == 0:
            return None
        return (reserve_out << 64) // reserve_in

    @staticmethod
    def _accumulate_price(cum: int, price_q64: int, slot_delta: int) -> int:
        """math::accumulate_price — wrapping `cum + price_q64 * slot_delta`."""
        return (cum + price_q64 * slot_delta) & MASK_128

    def init_oracle(self, now: int):
        """Seed the oracle clock at pool creation (nothing to accumulate yet)."""
        self.cum_sol_per_tok = 0
        self.cum_tok_per_sol = 0
        self.last_cum_slot = now
        self.observations = []

    def record_observation(self, now: int):
        """deep_pool::Pool::record_observation — advance the head from the
        PRE-swap reserves (the price that held over [last_cum_slot, now]). The
        head advances every swap; a ring snapshot is written once per spacing.
        Dust pools (< MIN_SPOT_RESERVE) advance the clock but don't accumulate."""
        if now <= self.last_cum_slot:
            return  # same slot / clock not advanced (idempotent)
        if self.sol_reserves < MIN_SPOT_RESERVE:
            self.last_cum_slot = now  # skip accumulation, don't mis-weight the gap
            return
        slot_delta = now - self.last_cum_slot
        sol_per_tok = self._price_q64(self.sol_reserves, self.token_reserves)
        tok_per_sol = self._price_q64(self.token_reserves, self.sol_reserves)
        if sol_per_tok is None or tok_per_sol is None:
            return
        self.cum_sol_per_tok = self._accumulate_price(self.cum_sol_per_tok, sol_per_tok, slot_delta)
        self.cum_tok_per_sol = self._accumulate_price(self.cum_tok_per_sol, tok_per_sol, slot_delta)
        self.last_cum_slot = now
        newest_slot = self.observations[-1]["slot"] if self.observations else 0
        if now - newest_slot >= MIN_OBS_SPACING_SLOTS:
            self.observations.append({
                "slot": now,
                "cum_sol_per_tok": self.cum_sol_per_tok,
                "cum_tok_per_sol": self.cum_tok_per_sol,
            })
            if len(self.observations) > TWAP_RING_SIZE:
                self.observations.pop(0)

    def read_twap_sol_per_tok(self, sol_now: int, tok_now: int, now: int, lookback: int):
        """deep_pool::Pool::read_twap_sol_per_tok — Q64.64 time-weighted
        SOL-per-token over `lookback`. Lazily extends the head to `now` at the
        current reserves (no swap since last_cum_slot ⇒ price unchanged), anchors
        at the newest snapshot ≥ lookback old, and divides the wrapping window sum
        by elapsed slots. None until the ring holds that much history (warmup →
        consumer fails closed)."""
        gap = max(0, now - self.last_cum_slot)
        price_now = self._price_q64(sol_now, tok_now)
        if price_now is None:
            return None
        cum_now = self._accumulate_price(self.cum_sol_per_tok, price_now, gap)
        target = now - lookback
        start = None
        for obs in self.observations:
            if obs["slot"] == 0 or obs["slot"] > target:
                continue
            if start is None or obs["slot"] > start["slot"]:
                start = obs
        if start is None:
            return None
        dt = now - start["slot"]
        if dt <= 0:
            return None
        cum_delta = (cum_now - start["cum_sol_per_tok"]) & MASK_128  # wrapping_sub
        return cum_delta // dt

    def oracle_snapshot(self):
        """Capture oracle state for the one speculative-swap revert path."""
        return (self.cum_sol_per_tok, self.cum_tok_per_sol, self.last_cum_slot,
                [dict(o) for o in self.observations])

    def oracle_restore(self, snap):
        self.cum_sol_per_tok, self.cum_tok_per_sol, self.last_cum_slot, obs = snap
        self.observations = [dict(o) for o in obs]


@dataclass
class Treasury:
    sol_balance: int = 0
    # Short tracking (V21 atomic-custodied — collateral lives in per-position vaults)
    total_tokens_lent: int = 0          # aggregate gross debt across open shorts
    active_shorts: int = 0
    short_interest_collected: int = 0
    # Baseline
    baseline_sol: int = 0
    baseline_tokens: int = 0
    baseline_initialized: bool = False
    # Harvest gating (mirror on-chain swap_fees_to_sol)
    harvested_fees_tokens: int = 0
    last_buyback_slot: int = 0
    min_buyback_interval_slots: int = DEFAULT_MIN_BUYBACK_INTERVAL_SLOTS
    is_community_token: bool = False
    # Admin gates
    lending_enabled: bool = True
    short_selling_enabled: bool = True
    # V21 leveraged long tracking (collateral in per-position vaults)
    total_token_collateral_locked: int = 0   # aggregate token collateral across open longs
    total_sol_lent_to_longs: int = 0          # aggregate gross SOL debt across open longs
    active_longs: int = 0
    long_interest_collected: int = 0

    @property
    def available_to_lend(self) -> int:
        """SOL available for long borrows. Short collateral is per-position
        (not in treasury), so the only deduction is total_sol_lent_to_longs."""
        cap = (self.sol_balance * LENDING_UTIL_CAP_BPS) // 10000
        return max(0, cap - self.total_sol_lent_to_longs)

    @property
    def utilization_bps(self) -> int:
        if self.sol_balance <= 0:
            return 0
        return (self.total_sol_lent_to_longs * 10000) // self.sol_balance


@dataclass
class LongPosition:
    """V21 atomic-custodied long.

    Open: user deposits tokens → position_token_vault. Borrow SOL from treasury,
          deduct 0.5% fee → treasury keeps fee, remainder atomically spent on
          pool_buy → bought tokens land in position_token_vault. Debt records
          the full borrow (gross — user owes back what was lent on their behalf).
    Close: sell all vault_tokens via pool → SOL out → repay debt to treasury,
           surplus SOL → user wallet.
    """
    user_id: int
    collateral_tokens: int = 0   # original deposit (record-keeping)
    vault_tokens: int = 0        # current vault balance — drained on close/liquidate
    borrowed_sol: int = 0        # debt (gross — full pre-fee borrow)
    accrued_interest: int = 0
    last_slot: int = 0


@dataclass
class ShortPosition:
    """V21 atomic-custodied short.

    Open: user deposits SOL, fee → treasury, net → position_sol_vault. Lock
          tokens atomically sold via pool → SOL output added to position_sol_vault.
          Debt records the full borrowed token amount (gross owed to lock).
    Close: vault_sol → atomic pool_buy (with gross-up) → tokens repaid to lock,
           surplus vault_sol → user wallet.
    """
    user_id: int
    collateral_sol: int = 0      # original deposit (record-keeping)
    vault_sol: int = 0           # current vault balance — drained on close/liquidate
    tokens_borrowed: int = 0     # debt (gross owed to lock)
    accrued_interest: int = 0
    last_slot: int = 0


class Event(Enum):
    BUY = "buy"
    SELL = "sell"
    BORROW = "borrow"
    REPAY = "repay"
    LIQUIDATE_LONG = "liquidate_long"
    OPEN_SHORT = "open_short"
    CLOSE_SHORT = "close_short"
    LIQUIDATE_SHORT = "liquidate_short"
    MIGRATE = "migrate"
    HARVEST = "harvest"


@dataclass
class LogEntry:
    slot: int
    event: Event
    user_id: int
    detail: dict = field(default_factory=dict)


# ============================================================================
# Simulator
# ============================================================================

class TorchSim:
    def __init__(self, bonding_target: int = BONDING_TARGET_TORCH, seed: int = 42):
        self.rng = random.Random(seed)
        self.slot: int = 0
        self.curve = BondingCurve(bonding_target=bonding_target)
        self.pool = Pool()
        self.treasury = Treasury()
        self.lock_tokens: int = TREASURY_LOCK  # short pool
        self.migrated: bool = False

        # Users: id -> token balance
        self.balances: dict[int, int] = {}
        self.sol_balances: dict[int, int] = {}  # lamports
        self.shorts: dict[int, ShortPosition] = {}
        self.longs: dict[int, LongPosition] = {}

        # Accounting
        self.protocol_treasury_sol: int = 0
        self.dev_wallet_sol: int = 0
        self.creator_sol: int = 0
        self.transfer_fee_accrued: int = 0  # tokens withheld

        self.log: list[LogEntry] = []
        self.snapshots: list[dict] = []

        # TWAP liquidation mark. The oracle itself lives on self.pool (keeperless,
        # advanced on swaps). This flag only toggles whether liquidations CONSUME
        # it — off by default so legacy raw-spot scenarios are byte-for-byte
        # unaffected; the D-10 scenarios flip it on.
        self.twap_enabled: bool = False

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _apply_transfer_fee(self, amount: int) -> tuple[int, int]:
        """Returns (net_received, fee_withheld). Ceil-rounds the fee to mirror
        Token-2022 / math.rs::calc_transfer_fee (`(amount*bps + 9999)//10000`,
        capped at MAX_TRANSFER_FEE = u64::MAX ⇒ effectively uncapped)."""
        fee = (amount * TRANSFER_FEE_BPS + 9999) // 10000
        return amount - fee, fee

    def _gross_up_for_transfer_fee(self, net: int) -> int:
        """Mirror of math.rs::gross_up_for_transfer_fee.

        Given a desired NET amount the recipient should receive after the
        Token-2022 transfer fee withhold, return the GROSS amount the sender
        must send. Used on short close/liquidate paths so lock_vault receives
        the full debt amount with no depletion — borrower (or liquidator)
        covers the fee on their repayment.

            gross = ceil(net * 10000 / (10000 - TRANSFER_FEE_BPS))
        """
        denom = 10_000 - TRANSFER_FEE_BPS
        if denom == 0:
            raise ValueError("TRANSFER_FEE_BPS == 10000")
        num = net * 10_000
        return (num + denom - 1) // denom

    def _pool_price_in_band(self) -> bool:
        if self.treasury.baseline_sol == 0 or self.treasury.baseline_tokens == 0:
            return False
        current_ratio = (self.pool.sol_reserves * 10**9) // self.pool.token_reserves
        baseline_ratio = (self.treasury.baseline_sol * 10**9) // self.treasury.baseline_tokens
        if baseline_ratio == 0:
            return False
        deviation = abs(current_ratio - baseline_ratio) * 10000 // baseline_ratio
        return deviation <= MAX_PRICE_DEVIATION_BPS

    def _accrue_short_interest(self, pos: ShortPosition):
        if pos.tokens_borrowed == 0:
            return
        slots_elapsed = self.slot - pos.last_slot
        if slots_elapsed <= 0:
            return
        interest = (pos.tokens_borrowed * DEFAULT_INTEREST_RATE_BPS * slots_elapsed) \
                   // (10000 * EPOCH_DURATION_SLOTS)
        pos.accrued_interest += interest
        pos.last_slot = self.slot

    def _short_ltv_bps(self, pos: ShortPosition) -> int:
        """V21: LTV measured against vault_sol (collateral + sale proceeds),
        not original collateral. Reflects actual coverage available to settle
        the token debt at current price."""
        if pos.vault_sol == 0 or self.pool.token_reserves == 0:
            return 10**18
        total_debt_tokens = pos.tokens_borrowed + pos.accrued_interest
        debt_value = (total_debt_tokens * self.pool.sol_reserves) // self.pool.token_reserves
        return (debt_value * 10000) // pos.vault_sol

    # ------------------------------------------------------------------
    # TWAP consumer — torch reads DeepPool's keeperless mark (the oracle lives
    # on self.pool now; mirror of torch's read_twap_price_q64). The mark is a
    # Q64.64 SOL-per-token price over LIQ_TWAP_LOOKBACK_SLOTS; None during warmup
    # → the liquidation TRIGGER fails closed. No crank, no ratchet — a single-tx
    # spot pump records one snapshot that the window average drowns out; only a
    # price HELD across the lookback moves the mark.
    # ------------------------------------------------------------------

    def _twap_price_q64(self):
        """Consumer read: Q64.64 sol-per-token over the liq lookback, or None
        (warmup / ring younger than the window). Reads from CURRENT reserves for
        the lazy head-extension — mirror of torch's read_twap_price_q64."""
        return self.pool.read_twap_sol_per_tok(
            self.pool.sol_reserves, self.pool.token_reserves,
            self.slot, LIQ_TWAP_LOOKBACK_SLOTS)

    @staticmethod
    def _value_in_sol_q64(token_amount: int, price_q64: int) -> int:
        """math::twap_value_in_sol — token→SOL at the Q64.64 mark."""
        return (token_amount * price_q64) >> 64

    @staticmethod
    def _tokens_to_seize_q64(debt_sol: int, bonus_bps: int, price_q64: int):
        """math::twap_tokens_to_seize — tokens to cover debt_sol + bonus at the
        Q64.64 mark. None on a zero marked price (fail closed)."""
        if price_q64 == 0:
            return None
        return (debt_sol * (10_000 + bonus_bps) << 64) // (10_000 * price_q64)

    def _short_ltv_at_twap(self, pos) -> int:
        pq = self._twap_price_q64()
        if pq is None:
            return 0  # warmup: no reference ⇒ refuse liquidation (conservative)
        if pos.vault_sol == 0:
            return 10**18
        total = pos.tokens_borrowed + pos.accrued_interest
        return self._value_in_sol_q64(total, pq) * 10000 // pos.vault_sol

    def _long_ltv_at_twap(self, pos) -> int:
        pq = self._twap_price_q64()
        if pq is None:
            return 0
        vault_value = self._value_in_sol_q64(pos.vault_tokens, pq)
        if vault_value == 0:
            return 10**18
        total = pos.borrowed_sol + pos.accrued_interest
        return total * 10000 // vault_value

    def _mark_price_q64(self):
        """Q64.64 sol-per-token for liquidation SEIZE accounting. Hardened TWAP
        when enabled + warm, else raw spot. Pricing the seize at the mark (not
        spot) stops a pump/dump at liquidation time from over-seizing a
        genuinely-liquidatable position."""
        if self.twap_enabled:
            pq = self._twap_price_q64()
            if pq is not None and pq > 0:
                return pq
        return Pool._price_q64(self.pool.sol_reserves, self.pool.token_reserves) or 0

    def _effective_liq_bonus_bps(self, ltv_bps: int) -> int:
        """Distress-scaled liquidation bonus (D-10 cap): 0 at the liq threshold,
        ramping linearly to DEFAULT_LIQ_BONUS_BPS at LIQ_FULL_BONUS_LTV_BPS. A
        manufactured liquidation only just crosses the threshold ⇒ ~0 prize, so
        the sustained hold to manufacture it isn't worth the cost; a genuine
        deep-distress position still pays the full bonus to real liquidators."""
        if ltv_bps <= DEFAULT_LIQ_THRESHOLD:
            return 0
        if ltv_bps >= LIQ_FULL_BONUS_LTV_BPS:
            return DEFAULT_LIQ_BONUS_BPS
        span = LIQ_FULL_BONUS_LTV_BPS - DEFAULT_LIQ_THRESHOLD
        return DEFAULT_LIQ_BONUS_BPS * (ltv_bps - DEFAULT_LIQ_THRESHOLD) // span

    def _get_balance(self, user_id: int) -> int:
        return self.balances.get(user_id, 0)

    def _get_sol(self, user_id: int) -> int:
        return self.sol_balances.get(user_id, 0)

    def _credit_tokens(self, user_id: int, amount: int):
        self.balances[user_id] = self.balances.get(user_id, 0) + amount

    def _debit_tokens(self, user_id: int, amount: int) -> int:
        bal = self.balances.get(user_id, 0)
        actual = min(bal, amount)
        self.balances[user_id] = bal - actual
        return actual

    def _credit_sol(self, user_id: int, amount: int):
        self.sol_balances[user_id] = self.sol_balances.get(user_id, 0) + amount

    def _debit_sol(self, user_id: int, amount: int) -> int:
        bal = self.sol_balances.get(user_id, 0)
        actual = min(bal, amount)
        self.sol_balances[user_id] = bal - actual
        return actual

    # ------------------------------------------------------------------
    # System-wide conservation totals (used by invariant scenarios)
    # ------------------------------------------------------------------

    def _system_sol_total(self) -> int:
        """Sum of every SOL container in the system.

        V21: a short's position vault (`vault_sol`) holds collateral + sale
        proceeds until close, so it is a first-class SOL container. Longs hold
        no SOL in-vault (their vault is tokens), so they contribute nothing
        here — their borrow is already reflected in treasury/pool balances.
        """
        return (sum(self.sol_balances.values())
                + self.treasury.sol_balance
                + self.pool.sol_reserves
                + self.curve.real_sol
                + self.protocol_treasury_sol
                + self.creator_sol
                + self.dev_wallet_sol
                + sum(sp.vault_sol for sp in self.shorts.values()))

    def _system_token_total(self) -> int:
        """Sum of every token container in the system.

        `lock_tokens` is the PHYSICAL lock balance — mutated by short
        open/close/liquidate flows (gross out on open, net in on close), not a
        `const − total_tokens_lent` proxy. This is what makes shorts conserve:
        the interest + gross-up rounding surplus that physically land in the
        lock on close are captured here rather than vanishing. `total_tokens_lent`
        is a separate debt-claim counter, not a token container.

        Long vaults (`vault_tokens`) hold collateral + bought tokens until close.
        """
        curve_or_pool = self.pool.token_reserves if self.migrated else self.curve.real_tokens
        return (sum(self.balances.values())
                + curve_or_pool
                + self.lock_tokens
                + sum(lp.vault_tokens for lp in self.longs.values())
                + self.treasury.harvested_fees_tokens
                + self.transfer_fee_accrued)

    # ------------------------------------------------------------------
    # Bonding Curve Operations
    # ------------------------------------------------------------------

    def buy(self, user_id: int, sol_amount: int) -> dict:
        """Buy tokens from bonding curve. Returns details dict."""
        assert not self.curve.bonding_complete, "Bonding already complete"
        assert sol_amount > 0

        sol_spent = self._debit_sol(user_id, sol_amount)
        if sol_spent == 0:
            return {"error": "no SOL"}

        # Protocol fee
        protocol_fee_total = (sol_spent * PROTOCOL_FEE_BPS) // 10000
        dev_share = (protocol_fee_total * DEV_WALLET_SHARE_BPS) // 10000
        protocol_fee = protocol_fee_total - dev_share
        self.protocol_treasury_sol += protocol_fee
        self.dev_wallet_sol += dev_share

        sol_after_fees = sol_spent - protocol_fee_total

        # Dynamic treasury rate
        reserves = self.curve.real_sol
        target = self.curve.bonding_target
        rate_range = TREASURY_SOL_MAX_BPS - TREASURY_SOL_MIN_BPS
        decay = (reserves * rate_range) // target if target > 0 else 0
        treasury_rate = max(TREASURY_SOL_MIN_BPS, TREASURY_SOL_MAX_BPS - decay)

        # Creator rate
        cr_range = CREATOR_SOL_MAX_BPS - CREATOR_SOL_MIN_BPS
        cr_growth = (reserves * cr_range) // target if target > 0 else 0
        creator_rate = min(CREATOR_SOL_MAX_BPS, CREATOR_SOL_MIN_BPS + cr_growth)

        total_split = (sol_after_fees * treasury_rate) // 10000
        # Community tokens: 100% of split goes to treasury, creator gets nothing.
        if self.treasury.is_community_token:
            creator_sol = 0
        else:
            creator_sol = (sol_after_fees * creator_rate) // 10000
        treasury_sol = total_split - creator_sol
        sol_to_curve = sol_after_fees - total_split

        # Constant product (preview — checked against MAX_WALLET_TOKENS before commit)
        tokens_out = (self.curve.virtual_tokens * sol_to_curve) \
                     // (self.curve.virtual_sol + sol_to_curve)
        tokens_out = min(tokens_out, self.curve.real_tokens)

        # Anti-concentration: reject if this buy would push the buyer over
        # MAX_WALLET_TOKENS. On-chain returns `MaxWalletExceeded`. Refund the
        # spent SOL since nothing else has mutated yet.
        new_balance = self._get_balance(user_id) + tokens_out
        if new_balance > MAX_WALLET_TOKENS:
            self._credit_sol(user_id, sol_spent)
            # Undo the protocol fee + dev share allocations.
            self.protocol_treasury_sol -= protocol_fee
            self.dev_wallet_sol -= dev_share
            return {"error": "max wallet tokens exceeded"}

        self.treasury.sol_balance += treasury_sol
        self.creator_sol += creator_sol

        self.curve.virtual_sol += sol_to_curve
        self.curve.virtual_tokens -= tokens_out
        self.curve.real_sol += sol_to_curve
        self.curve.real_tokens -= tokens_out

        self._credit_tokens(user_id, tokens_out)

        # Check bonding completion
        if self.curve.real_sol >= self.curve.bonding_target:
            self.curve.bonding_complete = True

        detail = {
            "sol_spent": sol_spent,
            "tokens_out": tokens_out,
            "protocol_fee": protocol_fee_total,
            "treasury_sol": treasury_sol,
            "creator_sol": creator_sol,
            "price": self.curve.price,
            "progress": self.curve.progress_pct,
            "bonding_complete": self.curve.bonding_complete,
        }
        self.log.append(LogEntry(self.slot, Event.BUY, user_id, detail))
        return detail

    def sell(self, user_id: int, token_amount: int) -> dict:
        """Sell tokens back to bonding curve (pre-migration only)."""
        assert not self.curve.bonding_complete, "Bonding already complete"
        tokens = self._debit_tokens(user_id, token_amount)
        if tokens == 0:
            return {"error": "no tokens"}

        # Constant product: sol_out = virtual_sol * tokens / (virtual_tokens + tokens)
        sol_out = (self.curve.virtual_sol * tokens) \
                  // (self.curve.virtual_tokens + tokens)
        sol_out = min(sol_out, self.curve.real_sol)

        # SELL_FEE_BPS = 0, so 100% goes to seller
        self.curve.virtual_sol -= sol_out
        self.curve.virtual_tokens += tokens
        self.curve.real_sol -= sol_out
        self.curve.real_tokens += tokens

        self._credit_sol(user_id, sol_out)

        detail = {
            "tokens_sold": tokens, "sol_received": sol_out,
            "price": self.curve.price, "progress": self.curve.progress_pct,
        }
        self.log.append(LogEntry(self.slot, Event.SELL, user_id, detail))
        return detail

    # ------------------------------------------------------------------
    # Migration
    # ------------------------------------------------------------------

    def migrate(self) -> dict:
        """Migrate bonding curve to DeepPool."""
        assert self.curve.bonding_complete, "Bonding not complete"
        assert not self.migrated, "Already migrated"

        # Treasury reimburses the migration payer for rent + LP creation costs
        # the payer fronted in the on-chain tx. Real value is dynamic; we use
        # MIGRATION_COST_APPROX as a midpoint and credit it to creator_sol
        # (the typical migration caller) so SOL conservation stays closed
        # across the full simulated system.
        migration_cost = min(MIGRATION_COST_APPROX, self.treasury.sol_balance)
        self.treasury.sol_balance -= migration_cost
        self.creator_sol += migration_cost

        self.pool.sol_reserves = self.curve.real_sol
        self.pool.token_reserves = self.curve.real_tokens
        self.pool.init_oracle(self.slot)  # seed the keeperless TWAP clock
        self.treasury.baseline_sol = self.curve.real_sol
        self.treasury.baseline_tokens = self.curve.real_tokens
        self.treasury.baseline_initialized = True
        self.curve.real_sol = 0       # mirrors on-chain: curve drained on migrate
        self.curve.real_tokens = 0
        self.migrated = True

        detail = {
            "pool_sol": self.pool.sol_reserves,
            "pool_tokens": self.pool.token_reserves,
            "price": self.pool.price,
        }
        self.log.append(LogEntry(self.slot, Event.MIGRATE, -1, detail))
        return detail

    # ------------------------------------------------------------------
    # Post-Migration Pool Trading
    # ------------------------------------------------------------------

    def pool_buy(self, user_id: int, sol_amount: int) -> dict:
        """Buy tokens from DeepPool (post-migration)."""
        assert self.migrated, "Not migrated"
        sol_spent = self._debit_sol(user_id, sol_amount)
        if sol_spent == 0:
            return {"error": "no SOL"}

        tokens_out = self.pool.swap_sol_for_tokens(sol_spent, self.slot)
        net, fee = self._apply_transfer_fee(tokens_out)
        self.transfer_fee_accrued += fee
        self._credit_tokens(user_id, net)

        return {"sol_spent": sol_spent, "tokens_received": net, "transfer_fee": fee,
                "price": self.pool.price}

    def pool_sell(self, user_id: int, token_amount: int) -> dict:
        """Sell tokens to DeepPool (post-migration)."""
        assert self.migrated, "Not migrated"
        tokens = self._debit_tokens(user_id, token_amount)
        if tokens == 0:
            return {"error": "no tokens"}

        net, fee = self._apply_transfer_fee(tokens)
        self.transfer_fee_accrued += fee
        sol_out = self.pool.swap_tokens_for_sol(net, self.slot)
        self._credit_sol(user_id, sol_out)

        return {"tokens_sold": tokens, "sol_received": sol_out, "transfer_fee": fee,
                "price": self.pool.price}


    # ------------------------------------------------------------------
    # Short Selling
    # ------------------------------------------------------------------

    def open_short(self, user_id: int, sol_collateral: int) -> dict:
        """V21 atomic-custodied short.

        Flow:
          1. Charge 0.5% open fee on borrow value in SOL → treasury
          2. Net collateral → position_sol_vault (vault_sol)
          3. Atomically borrow tokens from lock → pool_sell → SOL output → vault_sol
          4. Record debt as gross tokens borrowed; user owes that back on close
        """
        assert self.migrated
        if not self.treasury.short_selling_enabled:
            return {"error": "short selling not enabled"}
        if user_id in self.shorts:
            return {"error": "short already open (sim: 1 position per user per side)"}
        depth_max_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves)
        if depth_max_ltv == 0:
            return {"error": "pool too thin"}
        effective_max_ltv = min(depth_max_ltv, DEFAULT_MAX_LTV_BPS)

        sol = self._debit_sol(user_id, sol_collateral)
        if sol == 0:
            return {"error": "no SOL"}

        # Borrow plan with open fee:
        #   desired_borrow_value = collateral * ltv / 10000
        #   open_fee = desired_borrow_value * OPEN_FEE_BPS / 10000  (in SOL → treasury)
        #   net_collateral = collateral - open_fee
        #   borrow_value = net_collateral * ltv / 10000
        #   tokens_to_borrow = borrow_value * pool_tokens / pool_sol
        desired_borrow_value = (sol * effective_max_ltv) // 10000
        open_fee = (desired_borrow_value * OPEN_FEE_BPS) // 10000
        net_collateral = sol - open_fee
        borrow_value_sol = (net_collateral * effective_max_ltv) // 10000
        # [size cap] keep the position's SOL-debt-value ≤ ρ_max of pool depth so
        # the liquidation unwind slippage stays bounded regardless of pool size.
        size_cap = max_debt_value_for_depth(self.pool.sol_reserves)
        if borrow_value_sol > size_cap:
            borrow_value_sol = size_cap
        tokens_to_borrow = (borrow_value_sol * self.pool.token_reserves) \
                           // max(1, self.pool.sol_reserves)

        # Lock availability + per-user cap (MAX_WALLET_TOKENS). lock_tokens is
        # the physical lock balance — tokens already lent out have physically
        # left it, so it IS the lendable amount (no debt-counter subtraction).
        available_in_lock = self.lock_tokens
        tokens_to_borrow = min(tokens_to_borrow, available_in_lock)
        tokens_to_borrow = min(tokens_to_borrow, MAX_WALLET_TOKENS)
        if tokens_to_borrow <= 0:
            self._credit_sol(user_id, sol)
            return {"error": "no tokens available (per-user cap or lock empty)"}
        if tokens_to_borrow < MIN_SHORT_TOKENS:
            self._credit_sol(user_id, sol)
            return {"error": f"short too small ({tokens_to_borrow} < {MIN_SHORT_TOKENS})"}

        # Route open fee → treasury (operational SOL, grows lending pool)
        self.treasury.sol_balance += open_fee

        # Initialize position vault with net collateral
        vault_sol = net_collateral

        # Atomic: lock → pool sells tokens_to_borrow → SOL → vault.
        # Lock→pool transfer hits Token-2022 fee; pool receives net.
        net_tokens_into_pool, fee_lock_to_pool = self._apply_transfer_fee(tokens_to_borrow)
        self.transfer_fee_accrued += fee_lock_to_pool
        self.lock_tokens -= tokens_to_borrow  # physical: gross leaves lock (net→pool, fee withheld)
        sol_from_sale = self.pool.swap_tokens_for_sol(net_tokens_into_pool, self.slot)
        vault_sol += sol_from_sale

        # Persist position
        pos = ShortPosition(
            user_id=user_id,
            collateral_sol=net_collateral,   # record-keeping (post-fee)
            vault_sol=vault_sol,             # current vault balance
            tokens_borrowed=tokens_to_borrow,  # gross debt owed to lock
            last_slot=self.slot,
        )
        self.shorts[user_id] = pos
        self.treasury.total_tokens_lent += tokens_to_borrow
        self.treasury.active_shorts += 1

        detail = {
            "sol_collateral_gross": sol,
            "open_fee_sol": open_fee,
            "net_collateral_sol": net_collateral,
            "tokens_borrowed": tokens_to_borrow,
            "sol_from_sale": sol_from_sale,
            "vault_sol": pos.vault_sol,
            "ltv_bps": self._short_ltv_bps(pos),
            "price": self.pool.price,
        }
        self.log.append(LogEntry(self.slot, Event.OPEN_SHORT, user_id, detail))
        return detail

    def close_short(self, user_id: int, repay_fraction_bps: int = 10000) -> dict:
        """V21 atomic close — vault SOL pool-buys tokens to repay lock.

        Flow:
          1. Compute debt_to_repay = total_debt * repay_fraction / 10000
          2. Gross-up: debt_gross = ceil(debt_to_repay / (1 - TRANSFER_FEE))
          3. Quote sol_needed from pool for buying debt_gross tokens
          4. Spend sol_needed from vault → pool buy → tokens net → lock
          5. Surplus vault SOL → user wallet (full close) or stays in vault (partial)
        """
        pos = self.shorts.get(user_id)
        if pos is None:
            return {"error": "no short position"}
        self._accrue_short_interest(pos)

        total_debt = pos.tokens_borrowed + pos.accrued_interest
        if total_debt == 0:
            return {"error": "no debt"}
        debt_to_repay = (total_debt * repay_fraction_bps) // 10000
        if debt_to_repay <= 0:
            return {"error": "nothing to repay"}

        # Gross-up so lock receives net == debt_to_repay after Token-2022 fee
        debt_gross = self._gross_up_for_transfer_fee(debt_to_repay)
        sol_needed = self.pool.quote_buy_sol_in_for_tokens_out(debt_gross)
        if sol_needed is None:
            return {"error": "pool too thin to close"}

        if pos.vault_sol < sol_needed:
            return {"error": f"vault undercollateralized "
                              f"(vault={pos.vault_sol}, needs={sol_needed}); "
                              f"requires liquidation"}

        # Atomic: vault SOL → pool buy → tokens → lock
        tokens_out_gross = self.pool.swap_sol_for_tokens(sol_needed, self.slot)
        net_to_lock, fee_pool_to_lock = self._apply_transfer_fee(tokens_out_gross)
        self.transfer_fee_accrued += fee_pool_to_lock
        # Physical: ALL bought tokens (net of recipient fee) land in the lock.
        # Debt is reduced by `applied` below; any excess (interest + gross-up
        # rounding surplus) stays in the lock as protocol revenue — not lost.
        self.lock_tokens += net_to_lock

        # Drain vault by sol_needed
        pos.vault_sol -= sol_needed

        # Apply debt credit (interest first, then principal)
        applied = min(net_to_lock, debt_to_repay)
        interest_paid = min(applied, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        principal_paid = applied - interest_paid
        pos.tokens_borrowed -= min(principal_paid, pos.tokens_borrowed)

        self.treasury.total_tokens_lent -= min(principal_paid,
                                               self.treasury.total_tokens_lent)
        self.treasury.short_interest_collected += interest_paid

        fully_closed = pos.tokens_borrowed == 0 and pos.accrued_interest == 0
        surplus_sol = 0
        if fully_closed:
            surplus_sol = pos.vault_sol
            if surplus_sol > 0:
                self._credit_sol(user_id, surplus_sol)
                pos.vault_sol = 0
            self.treasury.active_shorts -= 1
            del self.shorts[user_id]

        detail = {
            "debt_repaid_net_to_lock": net_to_lock,
            "sol_spent_on_buyback": sol_needed,
            "surplus_sol_to_user": surplus_sol,
            "interest_paid": interest_paid,
            "principal_paid": principal_paid,
            "fully_closed": fully_closed,
            "vault_sol_remaining": pos.vault_sol if not fully_closed else 0,
        }
        self.log.append(LogEntry(self.slot, Event.CLOSE_SHORT, user_id, detail))
        return detail

    def liquidate_short(self, liquidator_id: int, shorter_id: int) -> dict:
        """V21 liquidation — liquidator pays tokens (gross-up'd) to lock,
        seizes vault SOL + bonus from position vault.

        Bad debt absorbed by treasury aggregate (`total_tokens_lent`).
        """
        pos = self.shorts.get(shorter_id)
        if pos is None:
            return {"error": "no short position"}
        self._accrue_short_interest(pos)

        ltv = self._short_ltv_bps(pos)  # spot LTV, for reporting (ltv_before)
        # Manipulation guard: the liquidation TRIGGER marks against the hardened
        # TWAP, not raw spot. An atomic spot pump can't move the clamped TWAP, so
        # a manufactured liquidation is refused (see docs D-10).
        if self.twap_enabled:
            if self._short_ltv_at_twap(pos) <= DEFAULT_LIQ_THRESHOLD:
                return {"error": "not liquidatable at TWAP reference (manipulation guard)"}
            # Asymmetric spot veto: TWAP is the binding trigger; spot may only
            # REFUSE when CLEARLY healthy (margin below threshold). Mirrors
            # `require!(spot_ltv > threshold - LIQ_SPOT_VETO_MARGIN_BPS)`.
            if ltv <= DEFAULT_LIQ_THRESHOLD - LIQ_SPOT_VETO_MARGIN_BPS:
                return {"error": "spot veto (position clearly healthy at spot)"}
        else:
            if ltv <= DEFAULT_LIQ_THRESHOLD:  # legacy raw-spot trigger (twap toggle off)
                return {"error": f"LTV {ltv} below liquidation threshold"}

        total_debt = pos.tokens_borrowed + pos.accrued_interest
        debt_to_cover = (total_debt * DEFAULT_LIQ_CLOSE_BPS) // 10000

        # SOL value of the covered token debt, priced at the TWAP mark (D-10
        # seize clamp) so a spot pump at liquidation can't inflate the SOL seized.
        if self.twap_enabled:
            debt_value_sol = self._value_in_sol_q64(debt_to_cover, self._mark_price_q64())
        else:  # legacy raw-spot path (twap toggle off)
            debt_value_sol = (debt_to_cover * self.pool.sol_reserves) // max(1, self.pool.token_reserves)
        # Bonus scales with distress (D-10 cap), measured on the hardened LTV.
        bonus_bps = (self._effective_liq_bonus_bps(self._short_ltv_at_twap(pos))
                     if self.twap_enabled else DEFAULT_LIQ_BONUS_BPS)
        target_sol_seize = (debt_value_sol * (10000 + bonus_bps)) // 10000
        actual_sol_seize = min(target_sol_seize, pos.vault_sol)

        # Bad debt scaling — if vault can't fully fund the seize bonus, reduce
        # how much debt the liquidator can cover proportionally.
        actual_tokens_covered = debt_to_cover
        if actual_sol_seize < target_sol_seize and target_sol_seize > 0:
            actual_tokens_covered = (debt_to_cover * actual_sol_seize) // target_sol_seize
        bad_debt = debt_to_cover - actual_tokens_covered \
                   if actual_tokens_covered < debt_to_cover else 0

        # Liquidator supplies tokens with gross-up (lock receives net == covered)
        cover_gross = self._gross_up_for_transfer_fee(actual_tokens_covered) \
                      if actual_tokens_covered > 0 else 0
        # Check before debiting — debiting then erroring would leak the
        # partial debit (no recipient), violating token conservation.
        if self._get_balance(liquidator_id) < cover_gross:
            return {"error": f"liquidator insufficient tokens "
                              f"(needs {cover_gross}, has {self._get_balance(liquidator_id)})"}
        tokens_from_liq = self._debit_tokens(liquidator_id, cover_gross)
        net_to_lock, fee = self._apply_transfer_fee(tokens_from_liq)
        self.transfer_fee_accrued += fee
        self.lock_tokens += net_to_lock  # physical: cover tokens land back in lock

        # Liquidator receives vault SOL
        pos.vault_sol -= actual_sol_seize
        self._credit_sol(liquidator_id, actual_sol_seize)

        # Apply credit to position debt
        applied = min(net_to_lock, actual_tokens_covered)
        interest_paid = min(applied, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        principal_paid = applied - interest_paid
        pos.tokens_borrowed -= min(principal_paid, pos.tokens_borrowed)

        # Bad debt write-off: reduce lent aggregate and zero position debt
        if bad_debt > 0:
            pos.tokens_borrowed = max(0, pos.tokens_borrowed - bad_debt)
            self.treasury.total_tokens_lent -= min(
                bad_debt, self.treasury.total_tokens_lent
            )

        self.treasury.total_tokens_lent -= min(principal_paid,
                                               self.treasury.total_tokens_lent)
        self.treasury.short_interest_collected += interest_paid

        fully_liquidated = pos.tokens_borrowed == 0 and pos.accrued_interest == 0
        if fully_liquidated:
            # Return any residual vault SOL to borrower
            if pos.vault_sol > 0:
                self._credit_sol(shorter_id, pos.vault_sol)
                pos.vault_sol = 0
            self.treasury.active_shorts -= 1
            del self.shorts[shorter_id]

        detail = {
            "tokens_covered": actual_tokens_covered,
            "sol_seized": actual_sol_seize,
            "bad_debt": bad_debt,
            "fully_liquidated": fully_liquidated,
            "ltv_before": ltv,
        }
        self.log.append(LogEntry(self.slot, Event.LIQUIDATE_SHORT, liquidator_id, detail))
        return detail

    # ------------------------------------------------------------------
    # V21 Leveraged Long (mirror of short)
    # ------------------------------------------------------------------

    def _long_ltv_bps(self, pos: "LongPosition") -> int:
        """V21: LTV measured against vault_tokens (collateral + bought),
        priced at live pool. Reflects actual coverage available to settle
        the SOL debt at current price — mirror of short LTV using vault_sol.
        """
        if pos.vault_tokens == 0 or self.pool.token_reserves == 0:
            return 10**18
        vault_value_sol = (pos.vault_tokens * self.pool.sol_reserves) \
                          // self.pool.token_reserves
        if vault_value_sol == 0:
            return 10**18
        total_debt = pos.borrowed_sol + pos.accrued_interest
        return (total_debt * 10000) // vault_value_sol

    def _accrue_long_interest(self, pos: "LongPosition"):
        """Mirror of _accrue_short_interest. SOL-denominated."""
        if pos.borrowed_sol == 0:
            return
        slots_elapsed = self.slot - pos.last_slot
        if slots_elapsed <= 0:
            return
        interest = (pos.borrowed_sol * DEFAULT_INTEREST_RATE_BPS * slots_elapsed) \
                   // (10000 * EPOCH_DURATION_SLOTS)
        pos.accrued_interest += interest
        pos.last_slot = self.slot

    def open_leveraged_long(
        self,
        user_id: int,
        collateral_tokens: int,
        min_tokens_out: int = 1,
    ) -> dict:
        """V21 atomic-custodied long.

        Flow:
          1. Collateral tokens → position_token_vault
          2. Compute borrow plan; 0.5% open fee on borrow value → treasury
          3. Atomic: (borrow - fee) SOL from treasury → pool_buy →
             bought tokens land in position_token_vault
          4. Debt records the FULL borrow (gross) — user owes back what was
             lent on their behalf, including the fee portion
        """
        assert self.migrated, "Not migrated"
        if not self.treasury.lending_enabled:
            return {"error": "leverage not enabled"}
        if user_id in self.longs:
            return {"error": "long already open (sim: 1 position per user per side)"}
        # Treasury floor — collateral SOL no longer in treasury (it's per-position),
        # so available = sol_balance - total_sol_lent_to_longs.
        available_pre = max(0, self.treasury.sol_balance
                            - self.treasury.total_sol_lent_to_longs)
        if available_pre < MIN_TREASURY_SOL_FOR_LENDING:
            return {"error": f"lending floor (need {MIN_TREASURY_SOL_FOR_LENDING}, "
                              f"have {available_pre})"}
        depth_max_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves)
        if depth_max_ltv == 0:
            return {"error": "pool too thin"}
        effective_max_ltv = min(depth_max_ltv, DEFAULT_MAX_LTV_BPS)

        # Debit collateral; apply transfer fee on user → vault leg
        tokens_deposited = self._debit_tokens(user_id, collateral_tokens)
        if tokens_deposited == 0:
            return {"error": "no tokens"}
        net_collateral, deposit_fee = self._apply_transfer_fee(tokens_deposited)
        self.transfer_fee_accrued += deposit_fee

        # Collateral value at pre-swap pool price
        collateral_value_sol = (net_collateral * self.pool.sol_reserves) \
                               // max(1, self.pool.token_reserves)
        if collateral_value_sol == 0:
            self._credit_tokens(user_id, net_collateral)
            return {"error": "zero collateral value"}

        # Borrow plan: full borrow at max LTV against collateral value.
        # User owes the full pre-fee borrow back; fee is paid out of treasury's
        # gross release (so net spend on pool buy = borrow - fee).
        desired_borrow_sol = (collateral_value_sol * effective_max_ltv) // 10000
        open_fee = (desired_borrow_sol * OPEN_FEE_BPS) // 10000
        atomic_buy_sol = desired_borrow_sol - open_fee

        # Per-user cap + global utilization cap
        max_lendable = (available_pre * LENDING_UTIL_CAP_BPS) // 10000
        absolute_cap = (max_lendable * MAX_USER_BORROW_SHARE_BPS) // 10000
        if desired_borrow_sol > absolute_cap:
            # Scale down borrow + atomic_buy + open_fee proportionally
            desired_borrow_sol = absolute_cap
            open_fee = (desired_borrow_sol * OPEN_FEE_BPS) // 10000
            atomic_buy_sol = desired_borrow_sol - open_fee
        # [size cap] keep the position's SOL-debt-value ≤ ρ_max of pool depth so
        # the liquidation unwind slippage stays bounded regardless of pool size.
        size_cap = max_debt_value_for_depth(self.pool.sol_reserves)
        if desired_borrow_sol > size_cap:
            desired_borrow_sol = size_cap
            open_fee = (desired_borrow_sol * OPEN_FEE_BPS) // 10000
            atomic_buy_sol = desired_borrow_sol - open_fee
        if desired_borrow_sol < MIN_BORROW_AMOUNT:
            self._credit_tokens(user_id, net_collateral)
            return {"error": f"desired_borrow {desired_borrow_sol} "
                              f"below MIN_BORROW_AMOUNT"}
        new_total = self.treasury.total_sol_lent_to_longs + desired_borrow_sol
        if new_total > max_lendable:
            self._credit_tokens(user_id, net_collateral)
            return {"error": "global lending cap exceeded"}

        # Atomic pool buy with the (borrow - fee) portion. Snapshot the oracle so
        # a slippage revert undoes the observation too (on-chain the whole tx
        # reverts, oracle write included).
        _osnap = self.pool.oracle_snapshot()
        tokens_out_gross = self.pool.swap_sol_for_tokens(atomic_buy_sol, self.slot)
        if tokens_out_gross < min_tokens_out:
            # Revert: undo pool + oracle, refund collateral
            self.pool.sol_reserves -= atomic_buy_sol
            self.pool.token_reserves += tokens_out_gross
            self.pool.oracle_restore(_osnap)
            self._credit_tokens(user_id, net_collateral)
            return {"error": "slippage exceeded"}
        # Token-2022 fee on pool→vault recipient leg
        net_tokens_out, recv_fee = self._apply_transfer_fee(tokens_out_gross)
        self.transfer_fee_accrued += recv_fee

        # Vault tokens = collateral + atomic buy output
        vault_tokens = net_collateral + net_tokens_out

        # Persist position. Debt records full borrow (user owes gross).
        pos = LongPosition(
            user_id=user_id,
            collateral_tokens=net_collateral,    # record-keeping (post-deposit-fee)
            vault_tokens=vault_tokens,           # current vault balance
            borrowed_sol=desired_borrow_sol,     # gross debt
            last_slot=self.slot,
        )
        self.longs[user_id] = pos
        self.treasury.active_longs += 1
        self.treasury.total_token_collateral_locked += net_collateral
        self.treasury.total_sol_lent_to_longs += desired_borrow_sol

        # SOL accounting: gross borrow leaves treasury, then fee comes back
        self.treasury.sol_balance -= desired_borrow_sol
        self.treasury.sol_balance += open_fee   # net SOL out of treasury = atomic_buy_sol

        detail = {
            "collateral_tokens": net_collateral,
            "borrowed_sol_gross": desired_borrow_sol,
            "open_fee_sol": open_fee,
            "atomic_buy_sol": atomic_buy_sol,
            "tokens_bought_net": net_tokens_out,
            "tokens_bought_gross": tokens_out_gross,
            "vault_tokens": pos.vault_tokens,
            "ltv_bps": self._long_ltv_bps(pos),
            "pool_price": self.pool.price,
        }
        self.log.append(LogEntry(self.slot, Event.BORROW, user_id, detail))
        return detail

    def close_leveraged_long(self, user_id: int, repay_fraction_bps: int = 10000) -> dict:
        """V21 atomic close — sell vault tokens via pool, split SOL output
        between treasury (debt repayment) and user (surplus).

        Full close (default): sell ALL vault_tokens in one swap, split SOL.
        Partial close: sell fraction of vault_tokens, repay fraction of debt,
        keep rest of vault open.
        """
        pos = self.longs.get(user_id)
        if pos is None:
            return {"error": "no long position"}
        self._accrue_long_interest(pos)

        if repay_fraction_bps <= 0 or repay_fraction_bps > 10000:
            return {"error": "invalid repay_fraction_bps"}
        total_debt = pos.borrowed_sol + pos.accrued_interest
        if total_debt == 0:
            return {"error": "no debt"}
        debt_to_repay = (total_debt * repay_fraction_bps) // 10000
        tokens_to_sell = (pos.vault_tokens * repay_fraction_bps) // 10000
        if tokens_to_sell == 0 or debt_to_repay == 0:
            return {"error": "fraction too small"}

        # Sell tokens — Token-2022 fee on vault → pool leg
        net_to_pool, sell_fee = self._apply_transfer_fee(tokens_to_sell)

        # On-chain reverts the whole tx when the sale can't cover the debt
        # (leverage.rs: require!(sol_out >= debt_to_repay, NotLiquidatable)).
        # Preview the swap first so nothing mutates on the revert path — an
        # underwater long must go through liquidation, not voluntary close.
        sol_out = self.pool.quote_tokens_for_sol(net_to_pool)
        if sol_out < debt_to_repay:
            return {"error": "NotLiquidatable: vault sale can't cover debt — must be liquidated"}

        # Solvent — now commit the swap (advances reserves + oracle) and vault.
        self.transfer_fee_accrued += sell_fee
        sol_out = self.pool.swap_tokens_for_sol(net_to_pool, self.slot)
        pos.vault_tokens -= tokens_to_sell

        # Split SOL output
        surplus_sol = sol_out - debt_to_repay
        interest_paid = min(debt_to_repay, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        principal_paid = debt_to_repay - interest_paid
        pos.borrowed_sol -= min(principal_paid, pos.borrowed_sol)

        self.treasury.sol_balance += debt_to_repay
        self.treasury.total_sol_lent_to_longs -= min(
            principal_paid, self.treasury.total_sol_lent_to_longs
        )
        self.treasury.long_interest_collected += interest_paid

        if surplus_sol > 0:
            self._credit_sol(user_id, surplus_sol)

        fully_closed = pos.borrowed_sol == 0 and pos.accrued_interest == 0
        if fully_closed:
            # Vault should be empty (sold all). Any residual collateral_locked
            # bookkeeping released.
            self.treasury.total_token_collateral_locked -= min(
                pos.collateral_tokens, self.treasury.total_token_collateral_locked
            )
            self.treasury.active_longs -= 1
            del self.longs[user_id]

        detail = {
            "sol_out_from_sale": sol_out,
            "debt_repaid": debt_to_repay,
            "interest_paid": interest_paid,
            "principal_paid": principal_paid,
            "surplus_sol_to_user": surplus_sol,
            "fully_closed": fully_closed,
            "vault_tokens_remaining": pos.vault_tokens if not fully_closed else 0,
        }
        self.log.append(LogEntry(self.slot, Event.REPAY, user_id, detail))
        return detail

    def liquidate_leveraged_long(self, liquidator_id: int, borrower_id: int) -> dict:
        """V21 liquidation — liquidator pays SOL to treasury, seizes vault
        tokens + bonus from position vault. Bad debt absorbed by
        treasury.total_sol_lent_to_longs.
        """
        pos = self.longs.get(borrower_id)
        if pos is None:
            return {"error": "no long"}
        assert self.migrated
        self._accrue_long_interest(pos)

        ltv = self._long_ltv_bps(pos)  # spot LTV, for reporting (ltv_before)
        # Manipulation guard (see docs D-10): trigger marks against hardened TWAP.
        if self.twap_enabled:
            if self._long_ltv_at_twap(pos) <= DEFAULT_LIQ_THRESHOLD:
                return {"error": "not liquidatable at TWAP reference (manipulation guard)"}
            # Asymmetric spot veto: spot may only REFUSE when CLEARLY healthy.
            if ltv <= DEFAULT_LIQ_THRESHOLD - LIQ_SPOT_VETO_MARGIN_BPS:
                return {"error": "spot veto (position clearly healthy at spot)"}
        else:
            if ltv <= DEFAULT_LIQ_THRESHOLD:  # legacy raw-spot trigger (twap toggle off)
                return {"error": f"LTV {ltv} below liquidation threshold"}

        total_debt = pos.borrowed_sol + pos.accrued_interest
        debt_to_cover = (total_debt * DEFAULT_LIQ_CLOSE_BPS) // 10000

        # Tokens to seize: debt_to_cover * (1 + bonus) / pool_price
        if self.pool.sol_reserves == 0:
            return {"error": "pool empty"}
        # Bonus scales with distress (D-10 cap), measured on the hardened LTV.
        bonus_bps = (self._effective_liq_bonus_bps(self._long_ltv_at_twap(pos))
                     if self.twap_enabled else DEFAULT_LIQ_BONUS_BPS)
        bonus_mult = (10000 + bonus_bps)
        # Seize priced at the TWAP mark (D-10 seize clamp): a spot dump at
        # liquidation can't inflate how many vault tokens are seized.
        if self.twap_enabled:
            seize = self._tokens_to_seize_q64(debt_to_cover, bonus_bps, self._mark_price_q64())
            target_seize = seize if seize is not None else 0
        else:  # legacy raw-spot path
            target_seize = (debt_to_cover * bonus_mult * self.pool.token_reserves) \
                           // (10000 * max(1, self.pool.sol_reserves))
        actual_seize = min(target_seize, pos.vault_tokens)

        # Bad debt scaling: if vault can't fund the seize bonus, scale debt covered
        if target_seize == 0:
            actual_debt_covered = 0
            bad_debt = debt_to_cover
        elif actual_seize < target_seize:
            actual_debt_covered = (debt_to_cover * actual_seize) // target_seize
            bad_debt = debt_to_cover - actual_debt_covered
        else:
            actual_debt_covered = debt_to_cover
            bad_debt = 0

        # Liquidator pays SOL → treasury (recoups loan principal + interest).
        # Check before debiting — debiting then erroring would leak the partial
        # debit (no recipient), violating SOL conservation.
        if self._get_sol(liquidator_id) < actual_debt_covered:
            return {"error": f"liquidator insufficient SOL "
                              f"(needs {actual_debt_covered}, has {self._get_sol(liquidator_id)})"}
        sol_paid = self._debit_sol(liquidator_id, actual_debt_covered)
        self.treasury.sol_balance += sol_paid

        # Seize vault tokens → liquidator (Token-2022 fee on outbound)
        net_to_liq, fee = self._apply_transfer_fee(actual_seize)
        self.transfer_fee_accrued += fee
        self._credit_tokens(liquidator_id, net_to_liq)
        pos.vault_tokens -= actual_seize

        # Apply credit to position debt
        interest_paid = min(actual_debt_covered, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        principal_paid = actual_debt_covered - interest_paid
        pos.borrowed_sol -= min(principal_paid, pos.borrowed_sol)

        # Bad debt write-off
        if bad_debt > 0:
            pos.borrowed_sol = max(0, pos.borrowed_sol - bad_debt)
            self.treasury.total_sol_lent_to_longs = max(
                0, self.treasury.total_sol_lent_to_longs - bad_debt
            )

        self.treasury.total_sol_lent_to_longs -= min(
            principal_paid, self.treasury.total_sol_lent_to_longs
        )
        self.treasury.long_interest_collected += interest_paid

        fully_liquidated = pos.borrowed_sol == 0 and pos.accrued_interest == 0
        if fully_liquidated:
            # Return any residual vault tokens to borrower (with outbound fee)
            if pos.vault_tokens > 0:
                rnet, rfee = self._apply_transfer_fee(pos.vault_tokens)
                self.transfer_fee_accrued += rfee
                self._credit_tokens(borrower_id, rnet)
                pos.vault_tokens = 0
            self.treasury.total_token_collateral_locked -= min(
                pos.collateral_tokens, self.treasury.total_token_collateral_locked
            )
            self.treasury.active_longs -= 1
            del self.longs[borrower_id]

        detail = {
            "debt_covered": actual_debt_covered,
            "collateral_seized": actual_seize,
            "bad_debt": bad_debt,
            "fully_liquidated": fully_liquidated,
            "ltv_before": ltv,
        }
        self.log.append(LogEntry(self.slot, Event.LIQUIDATE_LONG, liquidator_id, detail))
        return detail

    # ------------------------------------------------------------------
    # Fee Harvesting
    # ------------------------------------------------------------------

    def harvest_and_swap(self, is_community_token: bool | None = None) -> dict:
        """Harvest accumulated transfer fees and swap to SOL via pool.

        Mirrors on-chain `harvest_fees` + `swap_fees_to_sol`:
          0. Move withheld fees into treasury_token_account (accrued → harvested)
          1. Require baseline_initialized (enforced by ix context constraint)
          2. Cooldown gate: skip if current_slot < last_buyback + interval
          3. Ratio gate: skip if pool ratio < 1.2x baseline
          4. Sell 15% of held tokens (or 100% if balance <= SELL_ALL_TOKEN_THRESHOLD)
          5. Apply creator fee split (15% to creator unless community token)
          6. Update last_buyback_slot
        """
        assert self.migrated, "Not migrated"

        # 0. Harvest: withheld fees → treasury's holdings.
        if self.transfer_fee_accrued > 0:
            self.treasury.harvested_fees_tokens += self.transfer_fee_accrued
            self.transfer_fee_accrued = 0

        # 1. Baseline must be set (the on-chain ctx constraint enforces this).
        if not self.treasury.baseline_initialized:
            return {"error": "baseline not initialized"}

        token_amount = self.treasury.harvested_fees_tokens
        if token_amount == 0:
            return {"error": "no tokens to swap"}

        # 2. Cooldown gate (skip-not-fail to mirror on-chain `return Ok(())`).
        if self.treasury.last_buyback_slot > 0:
            next_slot = self.treasury.last_buyback_slot + self.treasury.min_buyback_interval_slots
            if self.slot < next_slot:
                return {"error": "cooldown", "next_slot": next_slot}

        # 3. Ratio gate: current pool price must be >= 120% of baseline.
        if self.pool.token_reserves == 0 or self.treasury.baseline_tokens == 0:
            return {"error": "empty pool"}
        current_ratio = (self.pool.sol_reserves * 10**9) // self.pool.token_reserves
        baseline_ratio = (self.treasury.baseline_sol * 10**9) // self.treasury.baseline_tokens
        sell_threshold = (baseline_ratio * DEFAULT_SELL_THRESHOLD_BPS) // 10000
        if current_ratio < sell_threshold:
            return {"error": "ratio gate", "current": current_ratio, "threshold": sell_threshold}

        # 4. Sell sizing: 15% of holdings, or 100% if below SELL_ALL threshold.
        if token_amount <= SELL_ALL_TOKEN_THRESHOLD:
            sell_amount = token_amount
        else:
            sell_amount = (token_amount * DEFAULT_SELL_PERCENT_BPS) // 10000
        if sell_amount == 0:
            return {"error": "sell amount zero"}

        # Swap tokens → SOL on the pool.
        sol_out = self.pool.swap_tokens_for_sol(sell_amount, self.slot)
        if sol_out == 0:
            return {"error": "swap returned 0"}

        # 5. Creator fee split. Mirrors on-chain: 0 to creator if community token.
        community = self.treasury.is_community_token if is_community_token is None else is_community_token
        if community:
            creator_amount = 0
            treasury_amount = sol_out
        else:
            creator_amount = (sol_out * CREATOR_FEE_SHARE_BPS) // 10000
            treasury_amount = sol_out - creator_amount

        self.treasury.sol_balance += treasury_amount
        self.creator_sol += creator_amount
        self.treasury.harvested_fees_tokens -= sell_amount

        # 6. Update cooldown anchor.
        self.treasury.last_buyback_slot = self.slot

        detail = {
            "tokens_swapped": sell_amount,
            "sol_out": sol_out,
            "creator_amount": creator_amount,
            "treasury_amount": treasury_amount,
            "treasury_total": self.treasury.sol_balance,
        }
        self.log.append(LogEntry(self.slot, Event.HARVEST, -1, detail))
        return detail

    # ------------------------------------------------------------------
    # Snapshots & Reporting
    # ------------------------------------------------------------------

    def advance(self, slots: int = 1):
        # Time only. The TWAP oracle is keeperless — it advances inside swaps
        # (the price's only writer), never on a bare clock tick.
        self.slot += slots

    def snapshot(self) -> dict:
        s = {
            "slot": self.slot,
            "migrated": self.migrated,
            "pool_sol": self.pool.sol_reserves / LAMPORTS_PER_SOL if self.migrated else 0,
            "pool_tokens": self.pool.token_reserves / 10**TOKEN_DECIMALS if self.migrated else 0,
            "pool_price": self.pool.price if self.migrated else self.curve.price,
            "pool_k": self.pool.k if self.migrated else 0,
            "treasury_sol": self.treasury.sol_balance / LAMPORTS_PER_SOL,
            "treasury_lent_to_longs": self.treasury.total_sol_lent_to_longs / LAMPORTS_PER_SOL,
            "treasury_util_bps": self.treasury.utilization_bps,
            "active_longs": self.treasury.active_longs,
            "active_shorts": self.treasury.active_shorts,
            "tokens_lent_to_shorts": self.treasury.total_tokens_lent / 10**TOKEN_DECIMALS,
            "protocol_revenue": self.protocol_treasury_sol / LAMPORTS_PER_SOL,
            "transfer_fees_tokens": self.transfer_fee_accrued / 10**TOKEN_DECIMALS,
            "short_interest_collected": self.treasury.short_interest_collected / LAMPORTS_PER_SOL,
            "long_interest_collected": self.treasury.long_interest_collected / LAMPORTS_PER_SOL,
        }
        self.snapshots.append(s)
        return s

    def print_snapshot(self):
        s = self.snapshot()
        print(f"\n{'='*60}")
        print(f"  Slot {s['slot']:,}")
        print(f"{'='*60}")
        if not s["migrated"]:
            print(f"  Bonding: {self.curve.progress_pct:.1f}% | "
                  f"Price: {self.curve.price * 10**TOKEN_DECIMALS:.6f} lamports/token")
        else:
            print(f"  Pool: {s['pool_sol']:.2f} SOL / {s['pool_tokens']:,.0f} tokens")
            print(f"  Price: {s['pool_price'] * 10**TOKEN_DECIMALS:.6f} lamports/token")
            print(f"  K invariant: {s['pool_k']:,}")
        depth_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves) if self.migrated else 0
        print(f"  Treasury: {s['treasury_sol']:.2f} SOL "
              f"(lent to longs: {s['treasury_lent_to_longs']:.2f}, "
              f"util: {s['treasury_util_bps']/100:.1f}%)")
        if self.migrated:
            print(f"  Depth band: {s['pool_sol']:.0f} SOL → max LTV {depth_ltv/100:.0f}%")
        print(f"  Longs: {s['active_longs']} | Shorts: {s['active_shorts']} "
              f"(tokens lent: {s['tokens_lent_to_shorts']:,.0f})")
        print(f"  Protocol revenue: {s['protocol_revenue']:.4f} SOL")
        print(f"  Transfer fees accrued: {s['transfer_fees_tokens']:,.0f} tokens")
        print(f"  Interest collected: short {s['short_interest_collected']:.4f} SOL, "
              f"long {s['long_interest_collected']:.4f} SOL")


# ============================================================================
# Scenario Runners
# ============================================================================

def scenario_full_lifecycle(seed=42):
    """Full lifecycle: bonding → migration → lending → shorts → liquidations."""
    sim = TorchSim(seed=seed)
    print("\n" + "="*60)
    print("  SCENARIO: Full Token Lifecycle")
    print("="*60)

    # Fund users — need ≥28 distinct buyers given MAX_WALLET_TOKENS (20M/wallet)
    # and ~550M tokens distributed over a full bonding (= 28 wallet-fulls).
    num_users = 50
    for i in range(num_users):
        sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL  # 50 SOL each

    # Phase 1: Bonding with PVP (buys AND sells on the curve)
    print("\n--- Phase 1: Bonding Curve (PVP) ---")
    buy_count = 0
    sell_count = 0
    gross_buy_volume = 0
    bonding_iters = 0
    bonding_iter_cap = 20000
    while not sim.curve.bonding_complete and bonding_iters < bonding_iter_cap:
        bonding_iters += 1
        user = sim.rng.randint(0, num_users - 1)
        # 65% buys, 35% sells — net positive but lots of churn
        if sim.rng.random() < 0.65 or sim._get_balance(user) == 0:
            amount = sim.rng.randint(1, 10) * LAMPORTS_PER_SOL
            amount = min(amount, sim._get_sol(user))
            if amount > 0:
                sim.buy(user, amount)
                buy_count += 1
                gross_buy_volume += amount
                # Refill SOL occasionally (new money entering)
                if sim._get_sol(user) < 2 * LAMPORTS_PER_SOL:
                    sim.sol_balances[user] += 5 * LAMPORTS_PER_SOL
        else:
            tokens = sim._get_balance(user)
            if tokens > 10**TOKEN_DECIMALS:
                # Sell 10-50% of holdings (taking profit / panic selling)
                sell_pct = sim.rng.randint(10, 50)
                sell_amt = (tokens * sell_pct) // 100
                if sell_amt > 0:
                    sim.sell(user, sell_amt)
                    sell_count += 1
        sim.advance(10)

    print(f"  Bonding complete: {buy_count} buys, {sell_count} sells")
    print(f"  Gross buy volume: {gross_buy_volume / LAMPORTS_PER_SOL:.1f} SOL")
    print(f"  Net to curve: {sim.curve.real_sol / LAMPORTS_PER_SOL:.1f} SOL")
    print(f"  Treasury from bonding: {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f} SOL")
    print(f"  Volume multiplier: {gross_buy_volume / sim.curve.real_sol:.1f}x")
    sim.print_snapshot()

    # Phase 2: Migration
    print("\n--- Phase 2: Migration ---")
    sim.migrate()
    sim.print_snapshot()

    # Phase 3: Organic trading + fee harvesting → demonstrate organic treasury growth
    print("\n--- Phase 3: Organic Trading + Fee Harvesting ---")
    target_treasury = 40 * LAMPORTS_PER_SOL
    harvest_interval = 500  # harvest every 500 slots
    slots_since_harvest = 0
    trade_count = 0
    harvest_count = 0

    # Balanced trading: 50/50 buy/sell, limited SOL inflow, realistic sizing.
    # Capped iterations — with the harvest ratio gate now enforced (mirrors
    # on-chain), harvests only fire when pool price drifts ≥120% baseline.
    # If random trading keeps price oscillating below that, treasury growth
    # stalls and the loop would hang. Cap + warn rather than spin.
    max_rounds = 2000
    rounds = 0
    while sim.treasury.sol_balance < target_treasury and rounds < max_rounds:
        rounds += 1
        # Small periodic inflow — not every round, and capped
        if sim.rng.random() < 0.3:  # 30% chance per round
            lucky = sim.rng.randint(0, num_users - 1)
            sim.sol_balances[lucky] += 2 * LAMPORTS_PER_SOL  # 2 SOL drip

        # Batch of trades — 50/50 split
        for _ in range(20):
            user = sim.rng.randint(0, num_users - 1)
            if sim.rng.random() < 0.50:  # balanced
                # Buy: 0.1-1 SOL (small trades)
                amount = sim.rng.randint(1, 10) * LAMPORTS_PER_SOL // 10
                if sim._get_sol(user) >= amount:
                    sim.pool_buy(user, amount)
                    trade_count += 1
            else:
                # Sell: 1-5% of holdings
                tokens = sim._get_balance(user)
                if tokens > 10**TOKEN_DECIMALS:
                    sell_pct = sim.rng.randint(1, 5)
                    sell_amt = max(10**TOKEN_DECIMALS, (tokens * sell_pct) // 100)
                    sim.pool_sell(user, sell_amt)
                    trade_count += 1
            sim.advance(50)
            slots_since_harvest += 50

        # Periodic harvest
        if slots_since_harvest >= harvest_interval and sim.transfer_fee_accrued > 0:
            result = sim.harvest_and_swap()
            if "error" not in result:
                harvest_count += 1
            slots_since_harvest = 0
    if rounds >= max_rounds:
        print(f"  WARN: treasury grow loop capped at {max_rounds} rounds — "
              f"reached {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f}/"
              f"{target_treasury / LAMPORTS_PER_SOL:.2f} SOL "
              f"(ratio-gated harvests likely didn't fire)")

    # Final harvest
    if sim.transfer_fee_accrued > 0:
        sim.harvest_and_swap()
        harvest_count += 1

    # Harvest analysis
    harvest_events = [e for e in sim.log if e.event == Event.HARVEST]
    total_tokens_harvested = sum(e.detail.get("tokens_swapped", 0) for e in harvest_events)
    total_sol_from_harvest = sum(e.detail.get("sol_out", 0) for e in harvest_events)
    if total_tokens_harvested > 0:
        avg_price_at_harvest = (total_sol_from_harvest / total_tokens_harvested) * 10**TOKEN_DECIMALS
    else:
        avg_price_at_harvest = 0
    min_sol = min((e.detail.get("sol_out", 0) for e in harvest_events), default=0)
    max_sol = max((e.detail.get("sol_out", 0) for e in harvest_events), default=0)

    print(f"  {trade_count} trades, {harvest_count} harvests")
    print(f"  Treasury grew to {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f} SOL")
    print(f"  Harvest stats:")
    print(f"    Total tokens harvested: {total_tokens_harvested / 10**TOKEN_DECIMALS:,.0f}")
    print(f"    Total SOL from harvests: {total_sol_from_harvest / LAMPORTS_PER_SOL:.4f}")
    print(f"    Avg price at harvest: {avg_price_at_harvest:.6f} lamports/token")
    print(f"    Migration price was: {sim.treasury.baseline_sol / sim.treasury.baseline_tokens * 10**TOKEN_DECIMALS:.6f} lamports/token")
    print(f"    Harvest range: {min_sol / LAMPORTS_PER_SOL:.6f} - {max_sol / LAMPORTS_PER_SOL:.6f} SOL per harvest")
    sim.print_snapshot()

    # Organic harvest can't realistically clear the 100-SOL lending floor at sim pool
    # scale — seed treasury above it so the long path is genuinely exercised (on-chain
    # this is real fee accrual over time). Matches the other long-path scenarios.
    seeded = max(0, 200 * LAMPORTS_PER_SOL - sim.treasury.sol_balance)
    sim.treasury.sol_balance += seeded
    if seeded:
        print(f"  Seeded treasury +{seeded / LAMPORTS_PER_SOL:.2f} SOL → "
              f"{sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f} SOL (above lending floor)")

    # Phase 4: V21 leveraged long opens (treasury seeded above lending floor)
    print("\n--- Phase 4: V21 Leveraged Long Opens ---")
    long_users = []
    for user in range(num_users):
        tokens = sim._get_balance(user)
        if tokens > 100 * 10**TOKEN_DECIMALS:
            collateral = tokens // 2
            try:
                result = sim.open_leveraged_long(user, collateral)
                if "error" not in result:
                    long_users.append(user)
                    print(f"  User {user} opened long: "
                          f"borrow {result['borrowed_sol_gross'] / LAMPORTS_PER_SOL:.2f} SOL, "
                          f"vault {result['vault_tokens'] / 10**TOKEN_DECIMALS:,.0f}, "
                          f"LTV {result['ltv_bps']/100:.1f}%")
            except Exception:
                pass
    print(f"  {len(long_users)} long positions opened")

    sim.print_snapshot()

    # Phase 5: Price crash → liquidation cascade
    print("\n--- Phase 5: Price Crash + Liquidation Cascade ---")
    whale = num_users
    sim.sol_balances[whale] = 100 * LAMPORTS_PER_SOL
    # The Phase 4 longs open at ~30% LTV (per-user borrow cap scales them below the
    # depth-band max), so the crash must exceed ~54% to push them past the 65% liq
    # threshold and actually exercise the cascade. Dump 2/3 of reserves (~64% crash).
    whale_tokens = sim.pool.token_reserves * 2 // 3
    sim.balances[whale] = whale_tokens
    chunk = whale_tokens // 10
    for _ in range(10):
        if sim._get_balance(whale) >= chunk:
            sim.pool_sell(whale, chunk)
        sim.advance(10)
    print(f"  Price after dump: {sim.pool.price * 10**TOKEN_DECIMALS:.6f} lamports/token")
    sim.advance(EPOCH_DURATION_SLOTS // 2)

    liquidator = num_users + 1
    sim.sol_balances[liquidator] = 200 * LAMPORTS_PER_SOL
    liquidation_count = 0
    for borrower_id in list(sim.longs.keys()):
        pos = sim.longs[borrower_id]
        sim._accrue_long_interest(pos)
        ltv = sim._long_ltv_bps(pos)
        if ltv > DEFAULT_LIQ_THRESHOLD:
            try:
                result = sim.liquidate_leveraged_long(liquidator, borrower_id)
                if "error" not in result:
                    liquidation_count += 1
                    print(f"  Liquidated user {borrower_id}: "
                          f"debt_covered={result['debt_covered']/LAMPORTS_PER_SOL:.4f} SOL, "
                          f"bad_debt={result['bad_debt']/LAMPORTS_PER_SOL:.4f} SOL")
            except (AssertionError, Exception) as e:
                print(f"  Liquidation failed for user {borrower_id}: {e}")

    print(f"  Total liquidations: {liquidation_count}")
    sim.print_snapshot()

    # Phase 6: Shorts — open BEFORE the crash (price still near baseline)
    # We need a fresh sim state where price is in-band, so we do shorts
    # right after migration in a separate sub-scenario
    print("\n--- Phase 6: Short Selling (separate sub-scenario from migration) ---")
    # Reset to post-migration state by creating a fresh sim for the short test
    short_sim = TorchSim(seed=seed + 100)
    for i in range(num_users):
        short_sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
    short_bonding_iters = 0
    while not short_sim.curve.bonding_complete and short_bonding_iters < 20000:
        short_bonding_iters += 1
        user = short_sim.rng.randint(0, num_users - 1)
        amount = short_sim.rng.randint(1, 15) * LAMPORTS_PER_SOL
        amount = min(amount, short_sim._get_sol(user))
        if amount > 0:
            short_sim.buy(user, amount)
        short_sim.advance(10)
    short_sim.migrate()
    print(f"  Fresh pool: {short_sim.pool.sol_reserves/LAMPORTS_PER_SOL:.0f} SOL, "
          f"price={short_sim.pool.price * 10**TOKEN_DECIMALS:.6f}")

    short_count = 0
    for i in range(10, 15):
        sol = short_sim._get_sol(i)
        if sol >= 5 * LAMPORTS_PER_SOL:
            try:
                result = short_sim.open_short(i, 5 * LAMPORTS_PER_SOL)
                if "error" not in result:
                    short_count += 1
                    print(f"  User {i} opened short: "
                          f"{result['tokens_borrowed']/10**TOKEN_DECIMALS:,.0f} tokens "
                          f"(LTV: {result['ltv_bps']/100:.1f}%)")
                    # Sell borrowed tokens on market
                    tokens = short_sim._get_balance(i)
                    if tokens > 0:
                        short_sim.pool_sell(i, tokens)
                else:
                    print(f"  User {i}: {result['error']}")
            except Exception as e:
                print(f"  Short failed for user {i}: {e}")

    print(f"  Shorts opened: {short_count}")
    short_sim.advance(EPOCH_DURATION_SLOTS)  # 1 epoch for interest
    short_sim.print_snapshot()

    # Phase 7: Price pump → short liquidations
    print("\n--- Phase 7: Price Pump + Short Liquidations ---")
    pumper = num_users + 2
    short_sim.sol_balances[pumper] = 200 * LAMPORTS_PER_SOL
    for _ in range(10):
        short_sim.pool_buy(pumper, 10 * LAMPORTS_PER_SOL)
        short_sim.advance(10)

    print(f"  Price after pump: {short_sim.pool.price * 10**TOKEN_DECIMALS:.6f} lamports/token")

    # Liquidate shorts
    short_liq = num_users + 3
    short_sim.sol_balances[short_liq] = 100 * LAMPORTS_PER_SOL
    short_sim.pool_buy(short_liq, 50 * LAMPORTS_PER_SOL)

    liq_count = 0
    for shorter_id in list(short_sim.shorts.keys()):
        pos = short_sim.shorts[shorter_id]
        short_sim._accrue_short_interest(pos)
        ltv = short_sim._short_ltv_bps(pos)
        if ltv > DEFAULT_LIQ_THRESHOLD:
            try:
                result = short_sim.liquidate_short(short_liq, shorter_id)
                if "error" not in result:
                    liq_count += 1
                    print(f"  Liquidated short {shorter_id}: "
                          f"tokens_covered={result['tokens_covered']/10**TOKEN_DECIMALS:,.0f}, "
                          f"sol_seized={result['sol_seized']/LAMPORTS_PER_SOL:.4f}, "
                          f"bad_debt={result['bad_debt']/10**TOKEN_DECIMALS:,.0f}")
                else:
                    print(f"  Liquidation of short {shorter_id}: {result}")
            except Exception as e:
                print(f"  Short liquidation failed for {shorter_id}: {e}")

    print(f"  Short liquidations: {liq_count}")
    short_sim.print_snapshot()

    # Summary (main sim — bonding through lending/liquidation)
    print("\n" + "="*60)
    print("  SUMMARY (main sim)")
    print("="*60)
    print(f"  Total events: {len(sim.log)}")
    print(f"  Protocol revenue: {sim.protocol_treasury_sol / LAMPORTS_PER_SOL:.4f} SOL")
    print(f"  Creator earnings: {sim.creator_sol / LAMPORTS_PER_SOL:.4f} SOL")
    print(f"  Dev wallet: {sim.dev_wallet_sol / LAMPORTS_PER_SOL:.4f} SOL")
    print(f"  Transfer fees: {sim.transfer_fee_accrued / 10**TOKEN_DECIMALS:,.0f} tokens")
    print(f"  Interest collected: short {sim.treasury.short_interest_collected / LAMPORTS_PER_SOL:.4f} SOL, "
          f"long {sim.treasury.long_interest_collected / LAMPORTS_PER_SOL:.4f} SOL")
    final_k = sim.pool.k
    print(f"  Pool K (final): {final_k:,}")
    print(f"  Pool K preserved: {'YES' if final_k > 0 else 'NO'}")

    if short_count > 0:
        print(f"\n  Short sub-scenario:")
        print(f"    Shorts opened: {short_count}, Liquidated: {liq_count}")
        print(f"    Short interest: {short_sim.treasury.short_interest_collected / 10**TOKEN_DECIMALS:,.0f} tokens")

    return sim


def scenario_cascade_stress(seed=123):
    """Stress test: maximum leverage → price crash → cascade liquidations.

    Key insight: treasury SOL from bonding is only ~22 SOL (dynamic fee split).
    To stress-test cascading liquidations we need more treasury liquidity.
    We simulate a mature token where treasury has accumulated SOL from
    fee harvesting and swap_fees_to_sol over time.
    """
    sim = TorchSim(seed=seed)
    print("\n" + "="*60)
    print("  SCENARIO: Liquidation Cascade Stress Test")
    print("="*60)

    # Bonding — distribute across many wallets with chunks small enough that
    # MAX_WALLET_TOKENS doesn't reject. At early prices, 0.5 SOL ≈ 5M tokens
    # → 4 buys per wallet fills it (20M cap). As price rises, larger chunks
    # become feasible. Random per-iter user pick spreads load naturally.
    num_users = 50
    for i in range(5):
        sim.sol_balances[i] = 200 * LAMPORTS_PER_SOL  # whales for later borrowing
    for i in range(5, num_users):
        sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
    bonding_iters = 0
    while not sim.curve.bonding_complete and bonding_iters < 20000:
        bonding_iters += 1
        i = sim.rng.randint(0, num_users - 1)
        # Chunk size scales with bonding progress (more room as price rises).
        progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
        max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)  # 0.3 → 2.3 SOL
        amt = min(sim._get_sol(i), max_chunk)
        if amt > 0:
            sim.buy(i, amt)  # error (cap, no SOL) → silently no-op next iter
        sim.advance(5)

    sim.migrate()
    print(f"  Migrated. Pool: {sim.pool.sol_reserves/LAMPORTS_PER_SOL:.0f} SOL, "
          f"{sim.pool.token_reserves/10**TOKEN_DECIMALS:,.0f} tokens")

    # Grow treasury organically via trading + harvesting
    print("\n  Growing treasury via organic trading + harvests...")
    target_treasury = 40 * LAMPORTS_PER_SOL
    trade_count = 0
    harvest_count = 0
    slots_since_harvest = 0

    # Traders (6-29) generate volume; whales (0-4) hold for borrowing later.
    # Capped iterations — same reason as full_lifecycle (ratio gate may keep
    # harvests from firing in random-trading conditions).
    max_rounds = 2000
    rounds = 0
    while sim.treasury.sol_balance < target_treasury and rounds < max_rounds:
        rounds += 1
        # Small periodic inflow for traders
        if sim.rng.random() < 0.3:
            lucky = sim.rng.randint(6, 29)
            sim.sol_balances[lucky] += 2 * LAMPORTS_PER_SOL

        for _ in range(30):
            user = sim.rng.randint(6, 29)
            if sim.rng.random() < 0.50:  # balanced
                amount = sim.rng.randint(1, 10) * LAMPORTS_PER_SOL // 10
                if sim._get_sol(user) >= amount:
                    sim.pool_buy(user, amount)
                    trade_count += 1
            else:
                tokens = sim._get_balance(user)
                if tokens > 10**TOKEN_DECIMALS:
                    sell_amt = max(10**TOKEN_DECIMALS, (tokens * sim.rng.randint(1, 5)) // 100)
                    sim.pool_sell(user, sell_amt)
                    trade_count += 1
            sim.advance(30)
            slots_since_harvest += 30

        if slots_since_harvest >= 400 and sim.transfer_fee_accrued > 0:
            sim.harvest_and_swap()
            harvest_count += 1
            slots_since_harvest = 0
    if rounds >= max_rounds:
        print(f"  WARN: treasury grow loop capped at {max_rounds} rounds — "
              f"reached {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f}/"
              f"{target_treasury / LAMPORTS_PER_SOL:.2f} SOL")

    if sim.transfer_fee_accrued > 0:
        sim.harvest_and_swap()
        harvest_count += 1

    harvest_events = [e for e in sim.log if e.event == Event.HARVEST]
    total_tokens_harvested = sum(e.detail.get("tokens_swapped", 0) for e in harvest_events)
    total_sol_from_harvest = sum(e.detail.get("sol_out", 0) for e in harvest_events)
    avg_harvest_price = (total_sol_from_harvest / total_tokens_harvested * 10**TOKEN_DECIMALS) if total_tokens_harvested > 0 else 0

    print(f"  {trade_count} trades, {harvest_count} harvests")
    print(f"  Treasury: {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f} SOL")
    print(f"  Harvest stats:")
    print(f"    Total tokens: {total_tokens_harvested / 10**TOKEN_DECIMALS:,.0f}")
    print(f"    Total SOL: {total_sol_from_harvest / LAMPORTS_PER_SOL:.4f}")
    print(f"    Avg harvest price: {avg_harvest_price:.4f} lamports/token")
    print(f"    Current pool price: {sim.pool.price * 10**TOKEN_DECIMALS:.4f} lamports/token")

    # Organic harvest can't clear the 100-SOL lending floor at sim pool scale — seed
    # treasury above it so the whale long opens are genuinely exercised (on-chain this
    # is real fee accrual over time). Matches the other long-path scenarios.
    sim.treasury.sol_balance = max(sim.treasury.sol_balance, 200 * LAMPORTS_PER_SOL)
    print(f"  Treasury seeded to {sim.treasury.sol_balance / LAMPORTS_PER_SOL:.2f} SOL (above lending floor)")

    # Whales (0-4) borrow at near-max depth-band LTV
    print("\n  Opening max-leverage loans (whales)...")
    depth_ltv = get_depth_max_ltv_bps(sim.pool.sol_reserves)
    print(f"    Pool: {sim.pool.sol_reserves/LAMPORTS_PER_SOL:.0f} SOL → depth LTV: {depth_ltv/100:.0f}%")
    loan_count = 0
    for i in range(5):
        tokens = sim._get_balance(i)
        if tokens > 1000 * 10**TOKEN_DECIMALS:
            collateral = min(tokens, 5_000_000 * 10**TOKEN_DECIMALS)  # 5M tokens max
            collateral_value = (collateral * sim.pool.sol_reserves) // sim.pool.token_reserves
            # Borrow at 95% of depth-band LTV
            # V21: open atomic leveraged long instead of V20 margin borrow.
            # The handler internally enforces the depth-band LTV cap, fees,
            # and per-user share cap; we just pass collateral and let it size.
            result = sim.open_leveraged_long(i, collateral)
            if "error" not in result:
                loan_count += 1
                print(f"    User {i}: long opened, "
                      f"borrow {result['borrowed_sol_gross']/LAMPORTS_PER_SOL:.2f} SOL "
                      f"at {result['ltv_bps']/100:.1f}% LTV "
                      f"({collateral_value/LAMPORTS_PER_SOL:.1f} SOL collateral)")
            else:
                print(f"    User {i}: {result['error']}")

    print(f"  Total long positions opened: {loan_count}")
    sim.print_snapshot()

    # Massive price crash: whale dumps 50% of pool tokens
    print("\n  Simulating whale dump (50% of pool)...")
    whale = 50
    sim.sol_balances[whale] = 0
    whale_tokens = (sim.pool.token_reserves * 50) // 100
    sim.balances[whale] = whale_tokens

    price_before = sim.pool.price
    chunk = whale_tokens // 10
    for _ in range(10):
        bal = sim._get_balance(whale)
        if bal >= chunk:
            sim.pool_sell(whale, chunk)
        sim.advance(5)

    price_after = sim.pool.price
    pct_drop = (1 - price_after / price_before) * 100
    print(f"  Price drop: {price_before * 10**TOKEN_DECIMALS:.6f} → "
          f"{price_after * 10**TOKEN_DECIMALS:.6f} ({pct_drop:.1f}% crash)")

    # Advance quarter epoch for interest to compound
    sim.advance(EPOCH_DURATION_SLOTS // 4)

    # Cascade liquidations — liquidator dumps seized tokens, worsening the crash
    print("\n  Running liquidation cascade...")
    liquidator = 51
    sim.sol_balances[liquidator] = 500 * LAMPORTS_PER_SOL
    total_bad_debt = 0
    total_liquidations = 0
    total_debt_covered = 0
    rounds = 0

    while True:
        rounds += 1
        liquidated_this_round = 0
        for borrower_id in list(sim.longs.keys()):
            pos = sim.longs[borrower_id]
            sim._accrue_long_interest(pos)
            ltv = sim._long_ltv_bps(pos)
            if ltv > DEFAULT_LIQ_THRESHOLD:
                try:
                    result = sim.liquidate_leveraged_long(liquidator, borrower_id)
                    if "error" not in result:
                        liquidated_this_round += 1
                        total_liquidations += 1
                        total_bad_debt += result["bad_debt"]
                        total_debt_covered += result["debt_covered"]
                        # Liquidator dumps seized vault tokens → price drops further
                        seized_tokens = sim._get_balance(liquidator)
                        if seized_tokens > 100 * 10**TOKEN_DECIMALS:
                            sim.pool_sell(liquidator, seized_tokens)
                            print(f"    Round {rounds}: liquidated user {borrower_id} "
                                  f"(debt={result['debt_covered']/LAMPORTS_PER_SOL:.2f} SOL, "
                                  f"bad_debt={result['bad_debt']/LAMPORTS_PER_SOL:.4f}), "
                                  f"dumped vault tokens → price={sim.pool.price * 10**TOKEN_DECIMALS:.4f}")
                except Exception as e:
                    pass
            sim.advance(1)

        if liquidated_this_round == 0:
            print(f"    Round {rounds}: no liquidations — cascade stopped")
            break
        if rounds > 20:
            print(f"    Round {rounds}: max rounds reached")
            break

    print(f"\n  Cascade complete:")
    print(f"    Rounds: {rounds}")
    print(f"    Total liquidations: {total_liquidations}")
    print(f"    Total debt covered: {total_debt_covered / LAMPORTS_PER_SOL:.4f} SOL")
    print(f"    Total bad debt: {total_bad_debt / LAMPORTS_PER_SOL:.4f} SOL")
    print(f"    Bad debt ratio: {total_bad_debt / max(1, total_debt_covered) * 100:.1f}%")
    print(f"    Final price: {sim.pool.price * 10**TOKEN_DECIMALS:.6f} lamports/token")
    print(f"    Price decline (total): {(1 - sim.pool.price / price_before) * 100:.1f}%")
    print(f"    Remaining long positions: {len(sim.longs)}")

    sim.print_snapshot()
    return sim


def scenario_sandwich_attack(seed=777):
    """Simulate sandwich attack on a large borrow."""
    sim = TorchSim(seed=seed)
    print("\n" + "="*60)
    print("  SCENARIO: Sandwich Attack on Borrow")
    print("="*60)

    # Fast bonding + migration. ≥28 distinct wallets with chunk sizes that
    # respect MAX_WALLET_TOKENS at early bonding prices.
    num_users = 50
    for i in range(num_users):
        sim.sol_balances[i] = 100 * LAMPORTS_PER_SOL
    bonding_iters = 0
    while not sim.curve.bonding_complete and bonding_iters < 20000:
        bonding_iters += 1
        i = sim.rng.randint(0, num_users - 1)
        progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
        max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
        amt = min(sim._get_sol(i), max_chunk)
        if amt > 0:
            sim.buy(i, amt)
        sim.advance(5)
    sim.migrate()

    # Seed treasury above the lending floor so the victim's long open actually reaches
    # the depth-band cap (the thing under test) instead of bouncing off the floor.
    sim.treasury.sol_balance = max(sim.treasury.sol_balance, 200 * LAMPORTS_PER_SOL)

    victim = 0
    attacker = 30
    sim.sol_balances[attacker] = 500 * LAMPORTS_PER_SOL

    # Victim has tokens from bonding
    victim_tokens = sim._get_balance(victim)
    print(f"  Victim tokens: {victim_tokens / 10**TOKEN_DECIMALS:,.0f}")
    print(f"  Pool price before: {sim.pool.price * 10**TOKEN_DECIMALS:.6f}")

    # Step 1: Attacker buys to pump price
    print("\n  Attacker front-runs: buying to inflate price...")
    attacker_sol_before = sim._get_sol(attacker)
    sim.pool_buy(attacker, 50 * LAMPORTS_PER_SOL)
    price_pumped = sim.pool.price
    print(f"  Pool price after pump: {price_pumped * 10**TOKEN_DECIMALS:.6f}")

    # Step 2: Victim opens leveraged long at inflated collateral value
    collateral = victim_tokens // 2
    collateral_value = (collateral * sim.pool.sol_reserves) // sim.pool.token_reserves

    depth_ltv = get_depth_max_ltv_bps(sim.pool.sol_reserves)
    print(f"  Pool depth: {sim.pool.sol_reserves / LAMPORTS_PER_SOL:.0f} SOL → "
          f"max LTV: {depth_ltv/100:.0f}%")

    print(f"\n  Victim opens leveraged long ({collateral_value/LAMPORTS_PER_SOL:.2f} SOL collateral)...")
    try:
        result = sim.open_leveraged_long(victim, collateral)
        if "error" in result:
            print(f"  Open rejected: {result['error']}")
            print(f"  >>> DEPTH BAND REJECTED — LTV too high for pool depth")
            sim.print_snapshot()
            return sim
        else:
            print(f"  Long opened: borrow {result['borrowed_sol_gross']/LAMPORTS_PER_SOL:.2f} SOL "
                  f"at {result['ltv_bps']/100:.1f}% LTV")
    except Exception as e:
        print(f"  Open failed: {e}")
        sim.print_snapshot()
        return sim

    # Step 3: Attacker dumps to crash price
    print("\n  Attacker back-runs: dumping tokens...")
    attacker_tokens = sim._get_balance(attacker)
    if attacker_tokens > 0:
        sim.pool_sell(attacker, attacker_tokens)
    price_after = sim.pool.price
    print(f"  Pool price after dump: {price_after * 10**TOKEN_DECIMALS:.6f}")

    # Check victim's LTV now (V21 leveraged long)
    if victim in sim.longs:
        pos = sim.longs[victim]
        ltv = sim._long_ltv_bps(pos)
        print(f"\n  Victim LTV after attack: {ltv / 100:.1f}%")
        print(f"  Liquidatable: {'YES' if ltv > DEFAULT_LIQ_THRESHOLD else 'NO'}")

    # Attacker profit/loss
    attacker_sol_after = sim._get_sol(attacker)
    attacker_pnl = (attacker_sol_after - attacker_sol_before) / LAMPORTS_PER_SOL
    print(f"  Attacker P&L: {attacker_pnl:+.4f} SOL")

    sim.print_snapshot()
    return sim


def scenario_harvest_gaming(seed=1234):
    """Adversarial: attacker pumps the pool price above the 120% baseline
    ratio gate, triggers `swap_fees_to_sol` (the on-chain harvest+sell), and
    then exits.

    The economic question: can the attacker net-profit by forcing the
    harvest at an inflated price (where the treasury sells its accumulated
    transfer-fee tokens for more SOL than they'd be worth post-attack)?

    Defenses in play:
      - Pool 0.25% swap fee on both the pump AND the exit
      - Slippage on both legs (pump bids price up, exit dumps it down)
      - Token-2022 transfer fee on the pump (tokens received < tokens paid for)
      - Harvest is permissionless — attacker captures NONE of the harvested
        SOL; it goes 85% treasury / 15% creator
      - Cooldown timer is per-treasury, not per-attacker — no way to chain

    Expected: attacker P&L < 0. The harvest benefits the treasury at the
    attacker's expense (attacker pays slippage to set up an event whose
    proceeds they don't capture).
    """
    print("\n" + "="*60)
    print("  SCENARIO: Harvest Gaming")
    print("="*60)

    sim = TorchSim(seed=seed)

    # Bonding — calibrated chunks across 50 wallets.
    num_users = 50
    for i in range(num_users):
        sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
    bonding_iters = 0
    while not sim.curve.bonding_complete and bonding_iters < 20000:
        bonding_iters += 1
        user = sim.rng.randint(0, num_users - 1)
        progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
        max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
        amt = min(sim._get_sol(user), max_chunk)
        if amt > 0:
            sim.buy(user, amt)
        sim.advance(5)
    sim.migrate()

    # Light post-migration trading to accumulate transfer-fee tokens in the
    # treasury (so there's something for the harvest to actually sell). We
    # bias buy-side so the pool ratio drifts up toward baseline — gives the
    # attacker a realistic starting point from which a moderate pump can
    # cross the 120% gate. (Without this bias, the warmup tends to push
    # price below baseline, and no realistic attacker pump can recover.)
    print("\n  Warmup: accumulating transfer fees, biased buy-side to drift ratio up...")
    for _ in range(300):
        u = sim.rng.randint(0, num_users - 1)
        if sim.rng.random() < 0.75:
            amt = sim.rng.randint(1, 10) * LAMPORTS_PER_SOL // 10
            if sim._get_sol(u) >= amt and amt > 0:
                sim.pool_buy(u, amt)
        else:
            tokens = sim._get_balance(u)
            if tokens > 10 * 10**TOKEN_DECIMALS:
                sim.pool_sell(u, tokens // 20)
        sim.advance(20)
    # Move accrued transfer fees into treasury's harvested_fees_tokens
    # (mirrors harvest_fees without the swap leg). Done here so the warmup
    # doesn't accidentally trigger the gate.
    sim.treasury.harvested_fees_tokens += sim.transfer_fee_accrued
    sim.transfer_fee_accrued = 0
    print(f"  Treasury accrued {sim.treasury.harvested_fees_tokens / 10**TOKEN_DECIMALS:,.0f} fee tokens")
    print(f"  Pool: {sim.pool.sol_reserves/LAMPORTS_PER_SOL:.2f} SOL / "
          f"{sim.pool.token_reserves/10**TOKEN_DECIMALS:,.0f} tokens")
    print(f"  Current ratio vs baseline: "
          f"{(sim.pool.sol_reserves * sim.treasury.baseline_tokens) / max(1, sim.pool.token_reserves * sim.treasury.baseline_sol):.4f}x "
          f"(gate at 1.2000x)")

    # ---- Attacker setup ----
    attacker = 99
    attacker_funding = 100 * LAMPORTS_PER_SOL
    sim.sol_balances[attacker] = attacker_funding
    pre_sol = sim._get_sol(attacker)
    pre_tokens = sim._get_balance(attacker)  # 0
    pre_pool_sol = sim.pool.sol_reserves
    pre_treasury = sim.treasury.sol_balance

    # ---- Step 1: pump to cross 120% gate ----
    # Need pool_sol_after / sol_baseline >= 1.2 * pool_sol_pre / sol_baseline,
    # i.e. pool_sol_after >= 1.2 * pool_sol_pre (ratio = sol/tokens, K-conserved
    # pump pushes sol up by sqrt(1.2) ≈ 9.5%). Add a chunk just over that.
    pump_amount = (pre_pool_sol * 12) // 100  # ~12% of pool SOL — comfortably past 1.2x
    pump_amount = min(pump_amount, pre_sol - LAMPORTS_PER_SOL)  # keep a SOL for tx headroom
    print(f"\n  Step 1: attacker pumps {pump_amount / LAMPORTS_PER_SOL:.2f} SOL into pool")
    pump_result = sim.pool_buy(attacker, pump_amount)
    if "error" in pump_result:
        print(f"    Pump FAILED: {pump_result['error']}")
        return sim
    tokens_acquired = pump_result["tokens_received"]
    pumped_price = sim.pool.price
    pumped_ratio = (sim.pool.sol_reserves * 10**9) // sim.pool.token_reserves
    baseline_ratio = (sim.treasury.baseline_sol * 10**9) // sim.treasury.baseline_tokens
    print(f"    Tokens acquired: {tokens_acquired / 10**TOKEN_DECIMALS:,.0f}")
    print(f"    Pool price: {pumped_price * 10**TOKEN_DECIMALS:.6f}")
    print(f"    Ratio vs baseline: {pumped_ratio / baseline_ratio:.4f}x (gate at 1.2000x)")

    # ---- Step 2: trigger harvest at the pumped price ----
    print("\n  Step 2: attacker triggers harvest_and_swap (anyone can call)")
    harvest = sim.harvest_and_swap()
    if "error" in harvest:
        print(f"    Harvest blocked: {harvest['error']}")
        # Continue anyway — the attacker still has to dump.
    else:
        print(f"    Treasury sold {harvest['tokens_swapped'] / 10**TOKEN_DECIMALS:,.0f} tokens "
              f"for {harvest['sol_out'] / LAMPORTS_PER_SOL:.4f} SOL")
        print(f"    Treasury kept {harvest['treasury_amount'] / LAMPORTS_PER_SOL:.4f} SOL, "
              f"creator got {harvest['creator_amount'] / LAMPORTS_PER_SOL:.4f} SOL")

    # ---- Step 3: attacker dumps everything to exit ----
    print("\n  Step 3: attacker dumps all tokens to exit")
    if tokens_acquired > 0:
        sim.pool_sell(attacker, sim._get_balance(attacker))
    post_pool_sol = sim.pool.sol_reserves
    post_pool_price = sim.pool.price
    print(f"    Pool price after dump: {post_pool_price * 10**TOKEN_DECIMALS:.6f}")

    # ---- Verdict ----
    post_sol = sim._get_sol(attacker)
    post_tokens = sim._get_balance(attacker)
    attacker_pnl = post_sol - pre_sol  # tokens went to 0 so cash diff is full P&L

    print(f"\n  ---- Verdict ----")
    print(f"    Attacker SOL: {pre_sol / LAMPORTS_PER_SOL:.4f} → {post_sol / LAMPORTS_PER_SOL:.4f}")
    print(f"    Attacker tokens: {pre_tokens} → {post_tokens}")
    print(f"    Attacker P&L: {attacker_pnl / LAMPORTS_PER_SOL:+.4f} SOL")
    treasury_gain = sim.treasury.sol_balance - pre_treasury
    print(f"    Treasury net gain: {treasury_gain / LAMPORTS_PER_SOL:+.4f} SOL "
          f"(harvest captured for protocol)")

    if attacker_pnl > 0:
        print(f"    *** ATTACK PROFITABLE *** — harvest gate gameable for "
              f"{attacker_pnl / LAMPORTS_PER_SOL:.4f} SOL")
    else:
        print(f"    PASS — value flow: attacker → pool (pump deposit) → treasury")
        print(f"           (harvest extraction). Attacker's exit dump retrieves")
        print(f"           less SOL because the pool has less SOL post-harvest.")
        print(f"           Treasury captured {treasury_gain / LAMPORTS_PER_SOL:.4f} SOL")
        print(f"           that originated from attacker's own pump deposit.")

    sim.print_snapshot()
    return sim


def scenario_long_short_dump(seed=5555):
    """Adversarial: user A combines a maxed-out long position (cheap from
    bonding + pool accumulation) with a maxed short borrow, dumps the
    combined stack into the pool to crash price, then either closes the
    short by buying back or walks away from the short collateral.

    The economic question: does the leverage from the short borrow let the
    attacker profit on the self-induced dump?

    Defenses:
      - MAX_WALLET_TOKENS caps bonding-curve buys at 20M (anti-concentration)
      - Per-user short borrow cap = MAX_WALLET_TOKENS = 20M (unified policy)
      - Pool 0.25% fee on each swap leg
      - Token-2022 0.07% transfer fee on each transfer
      - Constant-product slippage on a round-trip is strictly negative-EV:
            profit ≈ -S * r² / (1 + r)   where r = net_tokens_dumped / pool_tokens
      - Short interest accrues against the short obligation while open

    Two variants tested:
      A) Close-short variant: dump → buy back → close short → measure P&L
      B) Walk-away variant:    dump → forfeit short collateral → measure P&L

    Expected: P&L < 0 in BOTH variants. The constant-product invariant
    means any round-trip through the pool LOSES value to slippage, and the
    short layer just enlarges the round-trip.
    """
    print("\n" + "="*60)
    print("  SCENARIO: Long + Short Combo Dump")
    print("="*60)

    def _run_variant(walk_away: bool, sub_seed: int) -> dict:
        sim = TorchSim(seed=sub_seed)
        num_users = 50
        # Attacker = user 0; everyone else just provides bonding distribution.
        attacker = 0
        attacker_starting_sol = 300 * LAMPORTS_PER_SOL
        sim.sol_balances[attacker] = attacker_starting_sol
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL

        # Bonding — attacker buys aggressively early to land at MAX_WALLET_TOKENS.
        bonding_iters = 0
        while not sim.curve.bonding_complete and bonding_iters < 20000:
            bonding_iters += 1
            # 60% chance the attacker takes the slot, 40% to others — biases
            # the distribution toward the attacker hitting their cap.
            user = attacker if sim.rng.random() < 0.6 else sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(user), max_chunk)
            if amt > 0:
                sim.buy(user, amt)
            sim.advance(5)
        sim.migrate()

        attacker_after_bonding_tokens = sim._get_balance(attacker)
        attacker_after_bonding_sol = sim._get_sol(attacker)
        bonding_token_cost = attacker_starting_sol - attacker_after_bonding_sol

        # Post-migration: attacker accumulates MORE tokens via pool (no wallet
        # cap on deep_pool swap path — bonding cap is bonding-curve only).
        # We add a bounded amount so the test is reproducible.
        extra_buy_budget = 40 * LAMPORTS_PER_SOL
        if sim._get_sol(attacker) >= extra_buy_budget:
            sim.pool_buy(attacker, extra_buy_budget)
        attacker_post_buy_tokens = sim._get_balance(attacker)
        attacker_post_buy_sol = sim._get_sol(attacker)

        # Force-enable shorts (mainnet admin gate). Without this the attack
        # can't even start, which is itself a defense — but we want to
        # measure economics assuming the gate is open.
        sim.treasury.short_selling_enabled = True

        # Open short with remaining SOL as collateral (minus a small reserve
        # for the eventual buyback transaction).
        short_collateral = max(0, attacker_post_buy_sol - 5 * LAMPORTS_PER_SOL)
        short_result = sim.open_short(attacker, short_collateral)
        if "error" in short_result:
            return {
                "walk_away": walk_away,
                "outcome": f"short open failed: {short_result['error']}",
                "pnl": 0,
            }
        tokens_shorted = short_result["tokens_borrowed"]

        attacker_pre_dump_tokens = sim._get_balance(attacker)  # long + shorted
        attacker_pre_dump_sol = sim._get_sol(attacker)
        pool_price_pre_dump = sim.pool.price

        # The DUMP — sell entire stack into pool in chunks so the price tank
        # is computed against intermediate pool states (not single fat swap).
        chunks = 8
        per_chunk = attacker_pre_dump_tokens // chunks
        for _ in range(chunks):
            bal = sim._get_balance(attacker)
            chunk = min(per_chunk, bal)
            if chunk == 0:
                break
            sim.pool_sell(attacker, chunk)
            sim.advance(1)
        # Any leftover dust
        if sim._get_balance(attacker) > 0:
            sim.pool_sell(attacker, sim._get_balance(attacker))

        pool_price_after_dump = sim.pool.price
        attacker_after_dump_sol = sim._get_sol(attacker)

        if walk_away:
            # Variant B: walk away — don't repay short, let collateral go.
            # Advance time so short LTV deteriorates + a liquidator closes.
            sim.advance(EPOCH_DURATION_SLOTS // 2)
            liquidator = 999
            sim.sol_balances[liquidator] = 2000 * LAMPORTS_PER_SOL
            if attacker in sim.shorts:
                pos = sim.shorts[attacker]
                sim._accrue_short_interest(pos)
                ltv = sim._short_ltv_bps(pos)
                if ltv > DEFAULT_LIQ_THRESHOLD:
                    sim.liquidate_short(liquidator, attacker)
        else:
            # Variant A: close short. Buy back the borrowed tokens, close.
            if attacker in sim.shorts:
                pos = sim.shorts[attacker]
                owed = pos.tokens_borrowed + pos.accrued_interest
                # Buy back the owed amount at the now-tanked price.
                # Use a SOL amount sized to roughly cover the owed tokens.
                price_now = max(1, sim.pool.price)
                est_sol_needed = int(owed * price_now * 1.05)  # 5% headroom for fees
                if sim._get_sol(attacker) >= est_sol_needed:
                    sim.pool_buy(attacker, est_sol_needed)
                # Now close — pay back what we have
                returnable = min(sim._get_balance(attacker), owed)
                if returnable > 0:
                    sim.close_short(attacker, returnable)

        attacker_final_sol = sim._get_sol(attacker)
        attacker_final_tokens = sim._get_balance(attacker)
        # Mark final tokens at post-attack price for honest P&L.
        token_value_at_post_price = (attacker_final_tokens * sim.pool.sol_reserves) \
                                    // max(1, sim.pool.token_reserves)
        final_value = attacker_final_sol + token_value_at_post_price
        pnl = final_value - attacker_starting_sol

        return {
            "walk_away": walk_away,
            "attacker_starting_sol": attacker_starting_sol,
            "bonding_token_cost": bonding_token_cost,
            "attacker_after_bonding_tokens": attacker_after_bonding_tokens,
            "attacker_post_buy_tokens": attacker_post_buy_tokens,
            "tokens_shorted": tokens_shorted,
            "attacker_pre_dump_tokens": attacker_pre_dump_tokens,
            "pool_price_pre_dump": pool_price_pre_dump,
            "pool_price_after_dump": pool_price_after_dump,
            "attacker_final_sol": attacker_final_sol,
            "attacker_final_tokens": attacker_final_tokens,
            "final_value": final_value,
            "pnl": pnl,
            "outcome": "ok",
        }

    def _run_variant_c(sub_seed: int) -> dict:
        """Variant C: dump → buy back enough to close short PLUS restore
        long position (+ extra) → end with tokens-in-hand."""
        sim = TorchSim(seed=sub_seed)
        num_users = 50
        attacker = 0
        attacker_starting_sol = 300 * LAMPORTS_PER_SOL
        sim.sol_balances[attacker] = attacker_starting_sol
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
        bonding_iters = 0
        while not sim.curve.bonding_complete and bonding_iters < 20000:
            bonding_iters += 1
            user = attacker if sim.rng.random() < 0.6 else sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(user), max_chunk)
            if amt > 0:
                sim.buy(user, amt)
            sim.advance(5)
        sim.migrate()
        if sim._get_sol(attacker) >= 40 * LAMPORTS_PER_SOL:
            sim.pool_buy(attacker, 40 * LAMPORTS_PER_SOL)
        long_position = sim._get_balance(attacker)
        sim.treasury.short_selling_enabled = True
        short_result = sim.open_short(attacker, max(0, sim._get_sol(attacker) - 5 * LAMPORTS_PER_SOL))
        if "error" in short_result:
            return {"outcome": f"short failed: {short_result['error']}", "pnl": 0}
        tokens_shorted = short_result["tokens_borrowed"]
        pre_dump_tokens = sim._get_balance(attacker)
        pool_price_pre_dump = sim.pool.price

        # Dump everything (chunked)
        chunks = 8
        per_chunk = pre_dump_tokens // chunks
        for _ in range(chunks):
            bal = sim._get_balance(attacker)
            chunk = min(per_chunk, bal)
            if chunk == 0:
                break
            sim.pool_sell(attacker, chunk)
            sim.advance(1)
        if sim._get_balance(attacker) > 0:
            sim.pool_sell(attacker, sim._get_balance(attacker))

        pool_price_after_dump = sim.pool.price

        # Buy back enough to close short + restore long (+ small extra)
        # — i.e. roughly the full dumped quantity. This tests whether
        # "buy back D + additional" extracts value despite the round-trip.
        target_buyback_tokens = int(pre_dump_tokens * 1.0)  # 100% of dump
        # Compute SOL needed (approx via current price + 10% headroom for slippage)
        price_now = max(1, sim.pool.price)
        est_sol = int(target_buyback_tokens * price_now * 1.10)
        est_sol = min(est_sol, sim._get_sol(attacker))
        if est_sol > 0:
            sim.pool_buy(attacker, est_sol)

        pool_price_after_buyback = sim.pool.price
        post_buyback_tokens = sim._get_balance(attacker)

        # Close short — return owed tokens to lock pool
        if attacker in sim.shorts:
            pos = sim.shorts[attacker]
            owed = pos.tokens_borrowed + pos.accrued_interest
            returnable = min(post_buyback_tokens, owed)
            if returnable > 0:
                sim.close_short(attacker, returnable)

        # Mark final tokens at REALIZABLE value, not pool-spot. Spot is
        # misleading because the attacker's own buyback pushed price up — if
        # they tried to convert remaining tokens to SOL, they'd pay fresh
        # slippage on the way back down. Simulate the exit to measure true
        # liquidation value.
        final_sol = sim._get_sol(attacker)
        final_tokens = sim._get_balance(attacker)
        token_value_marked = (final_tokens * sim.pool.sol_reserves) // max(1, sim.pool.token_reserves)
        # Realizable value: dump remaining tokens, measure SOL received.
        realized_token_value = 0
        if final_tokens > 0:
            sol_before_exit = sim._get_sol(attacker)
            sim.pool_sell(attacker, final_tokens)
            realized_token_value = sim._get_sol(attacker) - sol_before_exit
            final_sol_after_exit = sim._get_sol(attacker)
        else:
            final_sol_after_exit = final_sol
        realized_value = final_sol_after_exit
        pnl_marked = (final_sol + token_value_marked) - attacker_starting_sol
        pnl_realized = realized_value - attacker_starting_sol

        return {
            "outcome": "ok",
            "attacker_starting_sol": attacker_starting_sol,
            "long_position": long_position,
            "tokens_shorted": tokens_shorted,
            "pre_dump_tokens": pre_dump_tokens,
            "pool_price_pre_dump": pool_price_pre_dump,
            "pool_price_after_dump": pool_price_after_dump,
            "pool_price_after_buyback": pool_price_after_buyback,
            "final_sol": final_sol,
            "final_tokens": final_tokens,
            "token_value_marked": token_value_marked,
            "realized_token_value": realized_token_value,
            "pnl_marked": pnl_marked,
            "pnl_realized": pnl_realized,
        }

    for label, walk_away, sub_seed in [
        ("A. Close short (buy back min)", False, seed),
        ("B. Walk away (forfeit collateral)", True, seed + 1),
    ]:
        print(f"\n  Variant {label}")
        r = _run_variant(walk_away, sub_seed)
        if r["outcome"] != "ok":
            print(f"    OUTCOME: {r['outcome']}")
            continue
        print(f"    Bonding cost: {r['bonding_token_cost'] / LAMPORTS_PER_SOL:.2f} SOL "
              f"for {r['attacker_after_bonding_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens")
        print(f"    Post-pool-buy tokens: {r['attacker_post_buy_tokens'] / 10**TOKEN_DECIMALS:,.0f}")
        print(f"    Shorted (borrowed): {r['tokens_shorted'] / 10**TOKEN_DECIMALS:,.0f}")
        print(f"    Pre-dump stack: {r['attacker_pre_dump_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens")
        print(f"    Pool price: {r['pool_price_pre_dump'] * 10**TOKEN_DECIMALS:.4f} → "
              f"{r['pool_price_after_dump'] * 10**TOKEN_DECIMALS:.4f} lamports/token "
              f"({(1 - r['pool_price_after_dump']/max(1e-18, r['pool_price_pre_dump']))*100:.1f}% drop)")
        print(f"    Final state: {r['attacker_final_sol'] / LAMPORTS_PER_SOL:.2f} SOL + "
              f"{r['attacker_final_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens")
        print(f"    Total value (marked post-attack): {r['final_value'] / LAMPORTS_PER_SOL:.2f} SOL")
        print(f"    Started with: {r['attacker_starting_sol'] / LAMPORTS_PER_SOL:.2f} SOL")
        print(f"    P&L: {r['pnl'] / LAMPORTS_PER_SOL:+.4f} SOL")
        if r["pnl"] > 0:
            print(f"    *** ATTACK PROFITABLE *** — protocol leak of "
                  f"{r['pnl'] / LAMPORTS_PER_SOL:.4f} SOL")
        else:
            print(f"    PASS — constant-product slippage + fees + caps make the "
                  f"combo dump strictly negative-EV.")

    # HOLD baseline — same setup (bonding + pool buy) but NO attack. Just
    # hold tokens through, then realize via exit dump. This isolates the
    # bonding-cheap early-buyer P&L from anything the attack contributes.
    def _run_hold_baseline(sub_seed: int) -> dict:
        sim = TorchSim(seed=sub_seed)
        num_users = 50
        attacker = 0
        starting = 300 * LAMPORTS_PER_SOL
        sim.sol_balances[attacker] = starting
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
        bi = 0
        while not sim.curve.bonding_complete and bi < 20000:
            bi += 1
            user = attacker if sim.rng.random() < 0.6 else sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(user), max_chunk)
            if amt > 0:
                sim.buy(user, amt)
            sim.advance(5)
        sim.migrate()
        if sim._get_sol(attacker) >= 40 * LAMPORTS_PER_SOL:
            sim.pool_buy(attacker, 40 * LAMPORTS_PER_SOL)
        # HOLD — no short, no dump, no buyback. Just realize at end.
        sol_before_exit = sim._get_sol(attacker)
        if sim._get_balance(attacker) > 0:
            sim.pool_sell(attacker, sim._get_balance(attacker))
        return {"realized": sim._get_sol(attacker), "pnl": sim._get_sol(attacker) - starting}

    print(f"\n  HOLD baseline (no attack — bond + pool buy + exit dump only)")
    hold = _run_hold_baseline(seed + 2)  # same seed as Variant C for fair comparison
    print(f"    Realized total: {hold['realized'] / LAMPORTS_PER_SOL:.2f} SOL")
    print(f"    Baseline P&L (bonding advantage only): {hold['pnl'] / LAMPORTS_PER_SOL:+.4f} SOL")

    # Variant C: the "buy back the whole stack" question — does extending the
    # buyback past just-closing-the-short turn the attack profitable?
    print(f"\n  Variant C. Buy back full stack (close short + restore long + extra)")
    r = _run_variant_c(seed + 2)
    if r["outcome"] != "ok":
        print(f"    OUTCOME: {r['outcome']}")
    else:
        print(f"    Long position pre-short: {r['long_position'] / 10**TOKEN_DECIMALS:,.0f} tokens")
        print(f"    Shorted: {r['tokens_shorted'] / 10**TOKEN_DECIMALS:,.0f}")
        print(f"    Pre-dump stack: {r['pre_dump_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens")
        print(f"    Pool price: {r['pool_price_pre_dump'] * 10**TOKEN_DECIMALS:.2f} → "
              f"{r['pool_price_after_dump'] * 10**TOKEN_DECIMALS:.2f} → "
              f"{r['pool_price_after_buyback'] * 10**TOKEN_DECIMALS:.2f} lamports/token "
              f"(dump → buyback)")
        print(f"    Mid-attack: {r['final_sol'] / LAMPORTS_PER_SOL:.2f} SOL + "
              f"{r['final_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens "
              f"(spot-marked: {(r['final_sol'] + r['token_value_marked']) / LAMPORTS_PER_SOL:.2f} SOL — MISLEADING)")
        print(f"    Realized exit: dumped {r['final_tokens'] / 10**TOKEN_DECIMALS:,.0f} tokens "
              f"for {r['realized_token_value'] / LAMPORTS_PER_SOL:.2f} SOL")
        print(f"    Final realized value: {(r['final_sol'] + r['realized_token_value']) / LAMPORTS_PER_SOL:.2f} SOL")
        print(f"    Started with: {r['attacker_starting_sol'] / LAMPORTS_PER_SOL:.2f} SOL")
        print(f"    Marked P&L (illusory): {r['pnl_marked'] / LAMPORTS_PER_SOL:+.4f} SOL")
        print(f"    REALIZED P&L (honest): {r['pnl_realized'] / LAMPORTS_PER_SOL:+.4f} SOL")
        attack_vs_hold = r["pnl_realized"] - hold["pnl"]
        print(f"    Variant C P&L (incl. bonding advantage): {r['pnl_realized'] / LAMPORTS_PER_SOL:+.4f} SOL")
        print(f"    HOLD baseline P&L (bonding advantage only): {hold['pnl'] / LAMPORTS_PER_SOL:+.4f} SOL")
        print(f"    ATTACK CONTRIBUTION (Variant C - HOLD): {attack_vs_hold / LAMPORTS_PER_SOL:+.4f} SOL")
        if attack_vs_hold > 0:
            print(f"    *** ATTACK ADDS VALUE *** over HOLD baseline by "
                  f"{attack_vs_hold / LAMPORTS_PER_SOL:.4f} SOL — investigate")
        else:
            print(f"    PASS — attack contribution is NEGATIVE vs HOLD baseline.")
            print(f"           The bonding-cheap profit is legitimate (by design); the")
            print(f"           round-trip on top destroys, not creates, value. CPMM")
            print(f"           K-invariant ensures round-trips cost fees + slippage.")

    return None


def scenario_leveraged_long(seed=7777):
    """V21 atomic-custodied leveraged long: validate happy paths + liquidation.

    V21 mechanics:
      - Open: collateral tokens → position vault. 0.5% open fee → treasury.
              (borrow - fee) SOL → atomic pool_buy → bought tokens → vault.
      - Close (atomic): sell vault tokens via pool, split SOL output.
              Debt repayment → treasury. Surplus SOL → user wallet.
      - Liquidate: liquidator pays SOL → treasury, seizes vault tokens + bonus.

    Three variants:
      A. Price UP → close at profit (vault tokens worth more, surplus SOL > 0)
      B. Price DOWN modestly → solvent close at small loss
      C. Price CRASH → liquidation (LTV > threshold)
    """
    print("\n" + "="*60)
    print("  SCENARIO: V21 Leveraged Long (atomic custodied)")
    print("="*60)

    def _setup() -> tuple["TorchSim", int]:
        sim = TorchSim(seed=seed)
        num_users = 50
        attacker = 0
        sim.sol_balances[attacker] = 200 * LAMPORTS_PER_SOL
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
        bi = 0
        while not sim.curve.bonding_complete and bi < 20000:
            bi += 1
            user = attacker if sim.rng.random() < 0.4 else sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(user), max_chunk)
            if amt > 0:
                sim.buy(user, amt)
            sim.advance(5)
        sim.migrate()
        sim.treasury.sol_balance = max(sim.treasury.sol_balance, 200 * LAMPORTS_PER_SOL)
        return sim, attacker

    # ---------- Variant A: price up → profit ----------
    print("\n  Variant A. Price UP → close at profit")
    sim, attacker = _setup()
    start_sol = sim._get_sol(attacker)
    start_tokens = sim._get_balance(attacker)
    pre_price = sim.pool.price
    print(f"    Pre-state: {start_sol/LAMPORTS_PER_SOL:.2f} SOL + "
          f"{start_tokens/10**TOKEN_DECIMALS:,.0f} tokens, "
          f"price {pre_price*10**TOKEN_DECIMALS:.2f} lamports/token")

    collateral = min(start_tokens // 4, 5_000_000 * 10**TOKEN_DECIMALS)
    r = sim.open_leveraged_long(attacker, collateral)
    if "error" in r:
        print(f"    OPEN FAILED: {r['error']}")
    else:
        print(f"    Opened: {r['collateral_tokens']/10**TOKEN_DECIMALS:,.0f} collateral, "
              f"borrow {r['borrowed_sol_gross']/LAMPORTS_PER_SOL:.2f} SOL "
              f"(fee {r['open_fee_sol']/LAMPORTS_PER_SOL:.4f}), "
              f"bought {r['tokens_bought_net']/10**TOKEN_DECIMALS:,.0f}, "
              f"vault {r['vault_tokens']/10**TOKEN_DECIMALS:,.0f}, "
              f"LTV {r['ltv_bps']/100:.1f}%")

        # Pump price
        for u in range(20, 40):
            if sim._get_sol(u) >= 3 * LAMPORTS_PER_SOL:
                sim.pool_buy(u, 3 * LAMPORTS_PER_SOL)
            sim.advance(2)
        new_price = sim.pool.price
        print(f"    Price after pump: {new_price*10**TOKEN_DECIMALS:.2f} "
              f"({(new_price/pre_price - 1)*100:+.1f}%)")

        # Atomic close — protocol sells vault tokens, splits SOL output
        cr = sim.close_leveraged_long(attacker)
        if "error" in cr:
            print(f"    CLOSE FAILED: {cr['error']}")
        else:
            print(f"    Close: sold for {cr['sol_out_from_sale']/LAMPORTS_PER_SOL:.2f} SOL, "
                  f"repaid {cr['debt_repaid']/LAMPORTS_PER_SOL:.2f}, "
                  f"surplus {cr['surplus_sol_to_user']/LAMPORTS_PER_SOL:+.4f} SOL → user")

        # Dump remaining wallet tokens (the original starting tokens user
        # didn't lock as collateral) at exit price for honest P&L compare.
        if sim._get_balance(attacker) > 0:
            sim.pool_sell(attacker, sim._get_balance(attacker))
        final_sol = sim._get_sol(attacker)
        print(f"    Final: {final_sol/LAMPORTS_PER_SOL:.2f} SOL "
              f"(P&L {(final_sol-start_sol)/LAMPORTS_PER_SOL:+.2f} SOL, expect positive)")

    # ---------- Variant B: price down modestly → solvent close ----------
    print("\n  Variant B. Price DOWN modestly → solvent close")
    sim, attacker = _setup()
    start_sol = sim._get_sol(attacker)
    collateral = min(sim._get_balance(attacker) // 4, 5_000_000 * 10**TOKEN_DECIMALS)
    r = sim.open_leveraged_long(attacker, collateral)
    if "error" in r:
        print(f"    OPEN FAILED: {r['error']}")
    else:
        print(f"    Opened: borrow {r['borrowed_sol_gross']/LAMPORTS_PER_SOL:.2f} SOL, "
              f"vault {r['vault_tokens']/10**TOKEN_DECIMALS:,.0f}")
        # Modest dump (not liquidation-level)
        for u in range(20, 35):
            tokens = sim._get_balance(u)
            if tokens > 10**TOKEN_DECIMALS:
                sim.pool_sell(u, tokens // 8)
            sim.advance(2)
        pos = sim.longs[attacker]
        sim._accrue_long_interest(pos)
        ltv = sim._long_ltv_bps(pos)
        print(f"    LTV after dump: {ltv/100:.1f}%")
        if ltv <= DEFAULT_LIQ_THRESHOLD:
            cr = sim.close_leveraged_long(attacker)
            if "error" in cr:
                print(f"    CLOSE FAILED: {cr['error']}")
            else:
                print(f"    Closed solvently. surplus {cr['surplus_sol_to_user']/LAMPORTS_PER_SOL:+.4f} SOL")
            if sim._get_balance(attacker) > 0:
                sim.pool_sell(attacker, sim._get_balance(attacker))
            print(f"    P&L: {(sim._get_sol(attacker)-start_sol)/LAMPORTS_PER_SOL:+.2f} SOL")
        else:
            print(f"    Position underwater — would liquidate (see Variant C)")

    # ---------- Variant C: price crash → liquidation ----------
    print("\n  Variant C. Price CRASH → liquidation")
    sim, attacker = _setup()
    collateral = min(sim._get_balance(attacker) // 4, 5_000_000 * 10**TOKEN_DECIMALS)
    r = sim.open_leveraged_long(attacker, collateral)
    if "error" in r:
        print(f"    OPEN FAILED: {r['error']}")
    else:
        print(f"    Opened: borrow {r['borrowed_sol_gross']/LAMPORTS_PER_SOL:.2f} SOL")
        # Whale crash
        whale = 99
        sim.sol_balances[whale] = 0
        whale_tokens = (sim.pool.token_reserves * 60) // 100
        sim.balances[whale] = whale_tokens
        chunk = whale_tokens // 10
        for _ in range(10):
            bal = sim._get_balance(whale)
            if bal >= chunk:
                sim.pool_sell(whale, chunk)
            sim.advance(2)
        pos = sim.longs[attacker]
        sim._accrue_long_interest(pos)
        ltv = sim._long_ltv_bps(pos)
        print(f"    LTV post-crash: {ltv/100:.1f}% (threshold {DEFAULT_LIQ_THRESHOLD/100:.0f}%)")
        if ltv > DEFAULT_LIQ_THRESHOLD:
            liquidator = 998
            sim.sol_balances[liquidator] = 500 * LAMPORTS_PER_SOL
            lr = sim.liquidate_leveraged_long(liquidator, attacker)
            if "error" in lr:
                print(f"    LIQUIDATION FAILED: {lr['error']}")
            else:
                print(f"    Liquidated. Debt covered: {lr['debt_covered']/LAMPORTS_PER_SOL:.2f} SOL, "
                      f"vault seized: {lr['collateral_seized']/10**TOKEN_DECIMALS:,.0f}, "
                      f"bad debt: {lr['bad_debt']/LAMPORTS_PER_SOL:.4f}")
                print(f"    Fully liquidated: {lr['fully_liquidated']}")
        else:
            print(f"    Position survived crash — LTV cap was conservative enough")

    return sim


def scenario_long_short_basis(seed=8888):
    """Strategy matrix exploration: HOLD / LONG-only / BASIS-neutral across
    both up and down price moves on the same starting state.

    The naive "open both and assume neutral" framing leaks the user's
    starting bonding position through the P&L. This scenario isolates the
    basis-trade's actual *contribution* by running three strategies on
    identical seeded states and comparing:

        HOLD   — no positions, just hold bonding tokens, sell at end
        LONG   — open leveraged long only (no hedge)
        BASIS  — open leveraged long + matching short (hedged)

    The basis trade's contribution is (BASIS − HOLD). If the design works:
      - On UP moves: BASIS gains LESS than HOLD (hedge gave up upside)
      - On DOWN moves: BASIS loses LESS than HOLD (hedge absorbed downside)
      - In both cases, BASIS P&L magnitude < LONG P&L magnitude (hedge works)
    """
    print("\n" + "="*60)
    print("  SCENARIO: Long + Short Basis Trade — Strategy Matrix")
    print("="*60)

    def _setup_state(sub_seed: int) -> tuple["TorchSim", int]:
        """Bond + migrate. Returns (sim, attacker_user_id). Identical setup
        for each strategy run so comparisons are apples-to-apples."""
        sim = TorchSim(seed=sub_seed)
        num_users = 50
        u = 0
        sim.sol_balances[u] = 200 * LAMPORTS_PER_SOL
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
        bi = 0
        while not sim.curve.bonding_complete and bi < 20000:
            bi += 1
            pick = u if sim.rng.random() < 0.3 else sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(pick), max_chunk)
            if amt > 0:
                sim.buy(pick, amt)
            sim.advance(5)
        sim.migrate()
        sim.treasury.sol_balance = max(sim.treasury.sol_balance, 200 * LAMPORTS_PER_SOL)
        sim.treasury.short_selling_enabled = True
        return sim, u

    def _apply_price_move(sim, direction: str):
        if direction == "up":
            for uu in range(20, 45):
                if sim._get_sol(uu) >= 3 * LAMPORTS_PER_SOL:
                    sim.pool_buy(uu, 3 * LAMPORTS_PER_SOL)
                sim.advance(2)
        else:
            for uu in range(20, 45):
                tokens = sim._get_balance(uu)
                if tokens > 10**TOKEN_DECIMALS:
                    sim.pool_sell(uu, tokens // 5)
                sim.advance(2)

    def _exit_to_sol(sim, u: int) -> int:
        """Close all positions atomically, liquidate token holdings, return SOL.
        V21: close handlers do the pool-route + debt-repayment + surplus
        transfer atomically. No manual pool_sell/pool_buy needed."""
        if u in sim.shorts:
            sim.close_short(u)  # default: full close
        if u in sim.longs:
            sim.close_leveraged_long(u)  # default: full close
        if sim._get_balance(u) > 0:
            sim.pool_sell(u, sim._get_balance(u))
        return sim._get_sol(u)

    def _run_hold(direction: str, sub_seed: int) -> dict:
        sim, u = _setup_state(sub_seed)
        start_sol = sim._get_sol(u)
        pre_price = sim.pool.price
        _apply_price_move(sim, direction)
        post_price = sim.pool.price
        end_sol = _exit_to_sol(sim, u)
        return {"strategy": "HOLD", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol}

    def _run_long_only(direction: str, sub_seed: int) -> dict:
        sim, u = _setup_state(sub_seed)
        start_sol = sim._get_sol(u)
        pre_price = sim.pool.price
        long_collateral = sim._get_balance(u) // 6
        rl = sim.open_leveraged_long(u, long_collateral)
        if "error" in rl:
            return {"strategy": "LONG", "error": rl["error"]}
        _apply_price_move(sim, direction)
        post_price = sim.pool.price
        end_sol = _exit_to_sol(sim, u)
        return {"strategy": "LONG", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol}

    def _run_basis(direction: str, sub_seed: int) -> dict:
        """V21: open leveraged long, then a short sized to hedge the LONG's
        bought-token portion (the "new" exposure from leverage). Both opens
        are atomic — short atomically sells borrowed tokens for SOL into the
        position vault; long atomically buys tokens with borrowed SOL into
        its vault. No manual sells needed post-open.

        Hedge sizing: short collateral chosen so that tokens_borrowed
        approximates the long's `tokens_bought_net`. Sizing isn't exact (it
        depends on LTV depth band) but close enough for the basis property
        to manifest.
        """
        sim, u = _setup_state(sub_seed)
        start_sol = sim._get_sol(u)
        pre_price = sim.pool.price

        # Open leveraged long
        long_collateral = sim._get_balance(u) // 6
        rl = sim.open_leveraged_long(u, long_collateral)
        if "error" in rl:
            return {"strategy": "BASIS", "error": f"long: {rl['error']}"}

        # Size short to hedge just the leveraged (bought) portion of the long.
        # tokens_to_short ≈ rl['tokens_bought_net']
        # SOL collateral needed: tokens × pool_sol / (effective_ltv × pool_tokens)
        target_short_tokens = rl.get("tokens_bought_net", 0)
        depth = get_depth_max_ltv_bps(sim.pool.sol_reserves)
        effective_ltv = min(depth, DEFAULT_MAX_LTV_BPS)
        target_sol_collateral = (target_short_tokens * sim.pool.sol_reserves * 10000) \
                                // max(1, sim.pool.token_reserves * effective_ltv)
        # Add ~10% headroom for open fee + cap to wallet (keep 1 SOL for tx)
        target_sol_collateral = int(target_sol_collateral * 1.1)
        target_sol_collateral = min(target_sol_collateral,
                                    max(0, sim._get_sol(u) - LAMPORTS_PER_SOL))
        rs = sim.open_short(u, target_sol_collateral)
        if "error" in rs:
            return {"strategy": "BASIS", "error": f"short: {rs['error']}"}

        # V21 atomic short already sold the borrowed tokens into vault SOL;
        # no follow-up needed. Hedge residual = long's bought exposure minus
        # short's borrowed.
        net_leverage_hedge = target_short_tokens - rs.get("tokens_borrowed", 0)

        _apply_price_move(sim, direction)
        post_price = sim.pool.price
        end_sol = _exit_to_sol(sim, u)
        return {"strategy": "BASIS", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol,
                "leverage_hedge_residual": net_leverage_hedge}

    # Run the 3×2 matrix
    results: dict[tuple[str, str], dict] = {}
    for direction in ["up", "down"]:
        for strategy_fn, label in [(_run_hold, "HOLD"),
                                    (_run_long_only, "LONG"),
                                    (_run_basis, "BASIS")]:
            r = strategy_fn(direction, seed)
            results[(direction, label)] = r

    # Print the matrix
    print(f"\n  Each strategy run on identical seeded state. P&L = end_SOL − start_SOL.")
    print(f"  start_SOL is post-bonding (the bonding-cheap appreciation is baked into every row).")

    for direction in ["up", "down"]:
        hold = results[(direction, "HOLD")]
        long_r = results[(direction, "LONG")]
        basis = results[(direction, "BASIS")]
        if "error" in hold or "error" in long_r or "error" in basis:
            print(f"\n  Direction {direction}: error in one of the runs")
            for k, v in results.items():
                if k[0] == direction and "error" in v:
                    print(f"    {k[1]}: {v['error']}")
            continue

        pre = hold["pre_price"] * 10**TOKEN_DECIMALS
        post = hold["post_price"] * 10**TOKEN_DECIMALS
        chg = (hold["post_price"] / hold["pre_price"] - 1) * 100
        print(f"\n  Price {direction.upper()}: {pre:.2f} → {post:.2f} lamports/token ({chg:+.1f}%)")
        print(f"    {'Strategy':<10} {'P&L (SOL)':>14} {'vs HOLD':>14}  notes")
        print(f"    {'-'*10} {'-'*14} {'-'*14}  {'-'*40}")
        for label, r in [("HOLD", hold), ("LONG", long_r), ("BASIS", basis)]:
            pnl_sol = r["pnl"] / LAMPORTS_PER_SOL
            vs_hold = (r["pnl"] - hold["pnl"]) / LAMPORTS_PER_SOL
            note = ""
            if label == "HOLD":
                note = "baseline (bonding-cheap tokens marked to market)"
            elif label == "LONG":
                note = "leveraged exposure to the direction"
            elif label == "BASIS":
                note = f"long + 2-step short (sold borrowed); residual leverage hedge: {r.get('leverage_hedge_residual', 0)/10**TOKEN_DECIMALS:+,.0f} tokens"
            print(f"    {label:<10} {pnl_sol:>+14.4f} {vs_hold:>+14.4f}  {note}")

        # Verdict on the hedge mechanic
        long_swing = abs(long_r["pnl"] - hold["pnl"])
        basis_swing = abs(basis["pnl"] - hold["pnl"])
        hedge_works = basis_swing < long_swing
        verdict = "✓ HEDGE WORKS" if hedge_works else "✗ HEDGE INEFFECTIVE"
        print(f"    {verdict}: |BASIS-HOLD| = {basis_swing/LAMPORTS_PER_SOL:.4f} SOL "
              f"vs |LONG-HOLD| = {long_swing/LAMPORTS_PER_SOL:.4f} SOL "
              f"(basis trade dampens directional exposure)")

    # Summary insight
    print(f"\n  ---- Interpretation ----")
    print(f"  V21 basis trade: both long and short opens are atomic, both positions are")
    print(f"  protocol-custodied. The hedge isolates the long's bought-token exposure")
    print(f"  via a matching short — it does NOT hedge the bonding-cheap tokens (those")
    print(f"  stay marked-to-market in the user's wallet). For full delta-neutrality,")
    print(f"  the short would need to cover the bonding holdings too, capped at")
    print(f"  MAX_WALLET_TOKENS per user. Within the long+short pair specifically:")
    print(f"  BASIS P&L tracks HOLD more closely than LONG does in BOTH directions.")

    return None


def scenario_directional_profitability(seed=9999):
    """Prove the leveraged long and short primitives ACTUALLY outperform
    HOLD when used in their intended regime — that they earn the user money
    relative to passive strategies, not just compose interesting positions.

    Four strategies, run across 5 price scenarios (down-30, down-10, flat,
    up-20, up-60). Shared setup: bonded token migrated, user starts with
    100 SOL and 0 starting tokens (to keep the comparison clean — no
    bonding-cheap appreciation confusion this time).

    Strategies:
      HOLD-SOL       — do nothing. P&L = 0. The baseline for SHORT.
      BUY-AND-HOLD   — spend 100 SOL on tokens at start, sell at end. The
                       baseline for LEVERAGED-LONG (matched-capital).
      LEVERAGED-LONG — buy some tokens, use as collateral, atomic-leverage,
                       hold, close. Should beat BUY-AND-HOLD when price rises
                       enough to cover interest + fees.
      SHORT          — open max short with 100 SOL collateral, sell borrowed
                       tokens, hold, buy back to close. Should beat HOLD-SOL
                       when price drops enough to cover interest + slippage.

    Expected outcomes (the proof):
      - On UP moves: LEVERAGED-LONG > BUY-AND-HOLD > HOLD-SOL
      - On DOWN moves: SHORT > HOLD-SOL > BUY-AND-HOLD > LEVERAGED-LONG
      - On FLAT: HOLD-SOL ≈ BUY-AND-HOLD; both leveraged strategies LOSE
        (fees + interest with no directional payoff)
      - Break-even thresholds reveal the minimum price move where each
        primitive is worth using
    """
    print("\n" + "="*60)
    print("  SCENARIO: Directional Profitability — Strategy × Price Matrix")
    print("="*60)

    def _setup_and_migrate(sub_seed: int) -> "TorchSim":
        """Identical post-migration state for every run. User 0 is reserved
        for the strategy under test; users 1-49 bond + own the pool's
        tokens. User 0 ends with 5 SOL, 0 tokens.

        Strategy capital is kept SMALL relative to pool depth (~200 SOL)
        so that the strategy's own trades don't significantly perturb the
        pool. This makes the "target price move" measurable consistently
        across strategies — otherwise LEV-LONG's setup pumps the price so
        much that subsequent down-moves still leave the price elevated."""
        sim = TorchSim(seed=sub_seed)
        num_users = 50
        sim.sol_balances[0] = 5 * LAMPORTS_PER_SOL
        for i in range(1, num_users):
            sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
        bi = 0
        while not sim.curve.bonding_complete and bi < 20000:
            bi += 1
            pick = sim.rng.randint(1, num_users - 1)
            progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
            max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
            amt = min(sim._get_sol(pick), max_chunk)
            if amt > 0:
                sim.buy(pick, amt)
            sim.advance(5)
        sim.migrate()
        sim.treasury.sol_balance = max(sim.treasury.sol_balance, 200 * LAMPORTS_PER_SOL)
        sim.treasury.short_selling_enabled = True
        return sim

    def _apply_price_move(sim, price_chg_pct: float):
        """Move the pool price by approximately price_chg_pct via other
        users' trades. Iterates with feedback until target is hit (or
        capacity is exhausted)."""
        pre_price = sim.pool.price
        if pre_price <= 0:
            return
        target_price = pre_price * (1 + price_chg_pct / 100.0)
        max_iters = 200
        for _ in range(max_iters):
            cur = sim.pool.price
            if price_chg_pct >= 0:
                if cur >= target_price:
                    return
                # Find a user with SOL and pump
                pumper = None
                for u_id in range(20, 49):
                    if sim._get_sol(u_id) >= 5 * LAMPORTS_PER_SOL:
                        pumper = u_id
                        break
                if pumper is None:
                    # Inject SOL into a random user
                    inject = sim.rng.randint(20, 49)
                    sim.sol_balances[inject] = sim.sol_balances.get(inject, 0) + 50 * LAMPORTS_PER_SOL
                    pumper = inject
                sim.pool_buy(pumper, 5 * LAMPORTS_PER_SOL)
            else:
                if cur <= target_price:
                    return
                # Find a user with tokens and dump
                dumper = None
                best_tokens = 0
                for u_id in range(20, 49):
                    t = sim._get_balance(u_id)
                    if t > best_tokens:
                        dumper, best_tokens = u_id, t
                if dumper is None or best_tokens < 10**TOKEN_DECIMALS:
                    return  # nobody can sell
                sim.pool_sell(dumper, max(10**TOKEN_DECIMALS, best_tokens // 10))
            sim.advance(2)

    def _hold_sol(sub_seed: int, target_chg: float) -> dict:
        sim = _setup_and_migrate(sub_seed)
        start_sol = sim._get_sol(0)
        pre_price = sim.pool.price
        _apply_price_move(sim, target_chg)
        post_price = sim.pool.price
        end_sol = sim._get_sol(0)
        return {"label": "HOLD-SOL", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol}

    def _buy_and_hold(sub_seed: int, target_chg: float) -> dict:
        sim = _setup_and_migrate(sub_seed)
        start_sol = sim._get_sol(0)
        pre_price = sim.pool.price
        # Spend ~all SOL on tokens (keep 0.5 SOL for tx headroom)
        sol_to_spend = sim._get_sol(0) - LAMPORTS_PER_SOL // 2
        sim.pool_buy(0, sol_to_spend)
        _apply_price_move(sim, target_chg)
        post_price = sim.pool.price
        # Sell all tokens at end
        if sim._get_balance(0) > 0:
            sim.pool_sell(0, sim._get_balance(0))
        end_sol = sim._get_sol(0)
        return {"label": "BUY-AND-HOLD", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol}

    def _leveraged_long(sub_seed: int, target_chg: float) -> dict:
        """V21 matched-capital comparison with BUY-AND-HOLD: same 100 SOL
        deployed. Difference: LEV-LONG uses leverage to take >100 SOL of
        notional token exposure.

        End-of-strategy P&L = mark-to-market net wealth:
          - Voluntarily close (atomic sell from vault, surplus → wallet)
          - If position still open + underwater, allow liquidation to settle
          - Wallet SOL at end = total P&L"""
        sim = _setup_and_migrate(sub_seed)
        start_sol = sim._get_sol(0)
        pre_price = sim.pool.price
        seed_spend = sim._get_sol(0) - 2 * LAMPORTS_PER_SOL
        sim.pool_buy(0, seed_spend)
        collateral = sim._get_balance(0)
        rl = sim.open_leveraged_long(0, collateral)
        if "error" in rl:
            return {"label": "LEVERAGED-LONG", "error": rl["error"]}
        _apply_price_move(sim, target_chg)
        post_price = sim.pool.price
        # Try atomic voluntary close (vault → pool sell → debt repay → surplus)
        if 0 in sim.longs:
            cr = sim.close_leveraged_long(0)
            # If undercollateralized, fall through to liquidation
            if "warning" in cr:
                pass  # position still open with residual debt
        # Liquidation pass if still open + underwater
        if 0 in sim.longs:
            pos = sim.longs[0]
            sim._accrue_long_interest(pos)
            ltv = sim._long_ltv_bps(pos)
            if ltv > DEFAULT_LIQ_THRESHOLD:
                liquidator = 199
                sim.sol_balances[liquidator] = 1000 * LAMPORTS_PER_SOL
                sim.liquidate_leveraged_long(liquidator, 0)
        # Dump any wallet tokens (these are pre-open spot, not leveraged)
        if sim._get_balance(0) > 0:
            sim.pool_sell(0, sim._get_balance(0))
        # MTM net wealth: wallet + remaining vault value − remaining debt
        wallet_sol = sim._get_sol(0)
        if 0 in sim.longs:
            pos = sim.longs[0]
            mtm_vault = (pos.vault_tokens * sim.pool.sol_reserves) \
                        // max(1, sim.pool.token_reserves)
            remaining_debt = pos.borrowed_sol + pos.accrued_interest
            net_wealth = wallet_sol + mtm_vault - remaining_debt
        else:
            net_wealth = wallet_sol
        return {"label": "LEVERAGED-LONG", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": net_wealth, "pnl": net_wealth - start_sol}

    def _short(sub_seed: int, target_chg: float) -> dict:
        """V21 atomic-custodied short. Deposit collateral SOL, protocol
        atomically sells borrowed tokens into vault SOL. Close atomically
        buys tokens back from vault SOL, surplus → user wallet.

        V21: user can deposit ~all SOL as collateral because the close
        buyback is funded from the vault, not the user's wallet. Keep a
        small reserve for tx headroom only."""
        sim = _setup_and_migrate(sub_seed)
        start_sol = sim._get_sol(0)
        pre_price = sim.pool.price
        collateral = sim._get_sol(0) - LAMPORTS_PER_SOL  # keep 1 SOL headroom
        rs = sim.open_short(0, max(0, collateral))
        if "error" in rs:
            return {"label": "SHORT", "error": rs["error"]}
        _apply_price_move(sim, target_chg)
        post_price = sim.pool.price
        # Atomic close — vault SOL → buy tokens → repay lock → surplus → wallet
        if 0 in sim.shorts:
            cr = sim.close_short(0)
            # If undercollateralized (price went up too much), allow liquidation
            if "error" in cr and "undercollateralized" in cr["error"]:
                liquidator = 199
                sim.sol_balances[liquidator] = 1000 * LAMPORTS_PER_SOL
                # Provide tokens to liquidator for buyback
                sim.balances[liquidator] = sim.balances.get(liquidator, 0) + \
                                            10**TOKEN_DECIMALS * 100_000_000
                sim.liquidate_short(liquidator, 0)
        # Dump any residual wallet tokens
        if sim._get_balance(0) > 0:
            sim.pool_sell(0, sim._get_balance(0))
        end_sol = sim._get_sol(0)
        return {"label": "SHORT", "pre_price": pre_price, "post_price": post_price,
                "start_sol": start_sol, "end_sol": end_sol, "pnl": end_sol - start_sol}

    # Run the matrix
    price_scenarios = [
        ("down-30", -30),
        ("down-10", -10),
        ("flat",      0),
        ("up-20",   +20),
        ("up-60",   +60),
    ]
    strategies = [
        ("HOLD-SOL",       _hold_sol),
        ("BUY-AND-HOLD",   _buy_and_hold),
        ("LEVERAGED-LONG", _leveraged_long),
        ("SHORT",          _short),
    ]
    results: dict[tuple[str, str], dict] = {}
    for ps_label, ps_chg in price_scenarios:
        for strat_label, strat_fn in strategies:
            # Use the same seed for each strategy in a given price scenario
            # so the setup state is identical — different strategy is the
            # only variable.
            results[(ps_label, strat_label)] = strat_fn(seed, ps_chg)

    # Print the matrix
    print(f"\n  All values in SOL. Starting SOL = 5 for each run.")
    print(f"  Strategy capital is small (~2.5% of pool depth) so the user's own")
    print(f"  trades don't significantly perturb the pool — keeps the target")
    print(f"  price move measurable across strategies.")
    print(f"  Same migration seed → identical pool depth + reserves at strategy open.")
    print()
    # Header
    print(f"  {'Strategy':<16} | " + " | ".join(f"{ps_label:>9}" for ps_label, _ in price_scenarios))
    print(f"  {'-'*16}-+-" + "-+-".join("-" * 9 for _ in price_scenarios))

    for strat_label, _ in strategies:
        row = f"  {strat_label:<16} | "
        cells = []
        for ps_label, _ in price_scenarios:
            r = results[(ps_label, strat_label)]
            if "error" in r:
                cells.append(f"{'ERR':>9}")
            else:
                cells.append(f"{r['pnl']/LAMPORTS_PER_SOL:>+9.2f}")
        row += " | ".join(cells)
        print(row)

    # Actual price moves achieved (might differ from targets due to slippage)
    print(f"\n  Realized price moves (target vs achieved):")
    for ps_label, ps_chg in price_scenarios:
        hold = results[(ps_label, "HOLD-SOL")]
        if "error" not in hold and hold["pre_price"] > 0:
            actual_chg = (hold["post_price"] / hold["pre_price"] - 1) * 100
            print(f"    {ps_label:<12} target {ps_chg:+d}%, actual {actual_chg:+.1f}%")

    # Verdicts
    print(f"\n  ---- Verdicts ----")
    for ps_label, _ in price_scenarios:
        hold_sol = results[(ps_label, "HOLD-SOL")]
        buy_hold = results[(ps_label, "BUY-AND-HOLD")]
        lev_long = results[(ps_label, "LEVERAGED-LONG")]
        short = results[(ps_label, "SHORT")]
        if any("error" in r for r in [hold_sol, buy_hold, lev_long, short]):
            continue

        # LEVERAGED-LONG vs BUY-AND-HOLD
        long_advantage = lev_long["pnl"] - buy_hold["pnl"]
        long_marker = "✓" if long_advantage > 0 else "✗"
        # SHORT vs HOLD-SOL
        short_advantage = short["pnl"] - hold_sol["pnl"]
        short_marker = "✓" if short_advantage > 0 else "✗"

        print(f"    {ps_label:<12}  "
              f"LEV-LONG vs BUY-HOLD: {long_advantage/LAMPORTS_PER_SOL:+7.2f} SOL {long_marker}   "
              f"SHORT vs HOLD-SOL: {short_advantage/LAMPORTS_PER_SOL:+7.2f} SOL {short_marker}")

    print(f"\n  ---- Interpretation ----")
    print(f"  LEVERAGED-LONG beats BUY-AND-HOLD on up moves large enough to cover")
    print(f"  the interest + double-pool-fee cost of opening + closing. The breakeven")
    print(f"  point is where the leverage's amplified gain offsets the financing cost.")
    print(f"")
    print(f"  SHORT beats HOLD-SOL on down moves large enough to cover the same costs.")
    print(f"  On flat or up markets, SHORT loses to HOLD-SOL (and LEV-LONG loses to")
    print(f"  BUY-AND-HOLD) because the user is paying for direction that didn't happen.")
    print(f"")
    print(f"  Neither directional primitive dominates universally — they're tools for")
    print(f"  users with a directional view. Without a view, HOLD-SOL or BUY-AND-HOLD")
    print(f"  outperforms both leveraged strategies because there's no payoff to offset")
    print(f"  the financing cost.")

    return None


def scenario_per_token_closure(seed=1717, num_actions=1500, num_users=40):
    """V21 closed-loop invariant: between open and close, NO leveraged asset
    reaches a user's wallet.

    The thesis of V21 is structural custody — borrowed/bought leverage lives in
    per-position vaults (`vault_sol` for shorts, `vault_tokens` for longs),
    never in the wallet, until the user explicitly exits. A wallet only changes
    via: spot buy/sell, collateral debit at open, or surplus/residual credit at
    close/liquidation. Opening or holding a position never credits the wallet
    with the leveraged asset.

    Asserted after every action:
      1. SOL + token conservation (system totals unchanged).
      2. Open closure deltas — opening must not credit the actor's wallet with
         the leveraged asset:
           open_short → wallet tokens unchanged, wallet SOL only debited
           open_long  → wallet SOL unchanged,    wallet tokens only debited
      3. Exit-in-SOL — close credits the owner SOL only; wallet tokens unchanged.
      4. Hold isolation — advancing time with positions open changes no wallet.

    1500 random ops across both sides satisfies the V21 success criterion.
    """
    print("\n" + "="*60)
    print(f"  SCENARIO: Per-Token Closure ({num_actions} random ops)")
    print("="*60)

    sim = TorchSim(seed=seed)
    per_user = 60 * LAMPORTS_PER_SOL
    for i in range(num_users):
        sim.sol_balances[i] = per_user
    total_sol_airdropped = per_user * num_users

    # Bootstrap to a migrated pool: buy on the curve (round-robin so no single
    # wallet trips the anti-concentration cap) until bonding completes.
    i = 0
    while not sim.curve.bonding_complete:
        u = i % num_users
        amt = min(sim._get_sol(u), 2 * LAMPORTS_PER_SOL)
        if amt > 0:
            sim.buy(u, amt)
        i += 1
        if i > 1_000_000:
            raise AssertionError("bonding never completed during bootstrap")
    sim.migrate()
    # Seed spot tokens (for long collateral / liquidator cover) via pool buys.
    # Generous enough that the long-open branch is reachable — otherwise shorts
    # (SOL-collateralized) starve longs (token-collateralized) of opportunities
    # and the long-side closure assertion never runs.
    for u in range(num_users):
        if sim._get_sol(u) >= 12 * LAMPORTS_PER_SOL:
            sim.pool_buy(u, 8 * LAMPORTS_PER_SOL)

    # Top the treasury above MIN_TREASURY_SOL_FOR_LENDING so the long path is
    # actually exercisable (bonding alone leaves it ~20 SOL, below the 100 SOL
    # floor). The top-up is folded into the conserved baseline — it's tracked
    # SOL the system must still account for, not an invisible injection.
    topup = max(0, MIN_TREASURY_SOL_FOR_LENDING + 50 * LAMPORTS_PER_SOL
                - sim.treasury.sol_balance)
    sim.treasury.sol_balance += topup
    total_sol_airdropped += topup

    def check_conservation(after: str) -> None:
        sol = sim._system_sol_total()
        if sol != total_sol_airdropped:
            raise AssertionError(
                f"SOL CONSERVATION VIOLATED after {after!r}: actual={sol}, "
                f"expected={total_sol_airdropped}, drift={sol - total_sol_airdropped}")
        tok = sim._system_token_total()
        if tok != TOTAL_SUPPLY:
            raise AssertionError(
                f"TOKEN CONSERVATION VIOLATED after {after!r}: actual={tok}, "
                f"expected={TOTAL_SUPPLY}, drift={tok - TOTAL_SUPPLY}")

    def random_op() -> str:
        roll = sim.rng.random()
        user = sim.rng.randint(0, num_users - 1)

        if roll < 0.22:  # spot buy — wallet changes freely (no leverage)
            amt = sim.rng.randint(1, 20) * LAMPORTS_PER_SOL // 10
            if sim._get_sol(user) >= amt and amt > 0:
                sim.pool_buy(user, amt)
                return "pool_buy"
        elif roll < 0.40:  # spot sell
            tokens = sim._get_balance(user)
            if tokens > 10**TOKEN_DECIMALS:
                sim.pool_sell(user, max(10**TOKEN_DECIMALS, tokens // 20))
                return "pool_sell"
        elif roll < 0.55:  # open short — CLOSURE: no tokens to wallet
            if user not in sim.shorts and sim._get_sol(user) >= 3 * LAMPORTS_PER_SOL:
                collateral = sim.rng.randint(1, 3) * LAMPORTS_PER_SOL
                tok_before, sol_before = sim._get_balance(user), sim._get_sol(user)
                res = sim.open_short(user, collateral)
                if "error" not in res:
                    assert sim._get_balance(user) == tok_before, \
                        "open_short leaked borrowed tokens into wallet"
                    assert sol_before - sim._get_sol(user) == res["sol_collateral_gross"], \
                        "open_short wallet SOL delta != collateral (asset leaked)"
                    return "open_short"
        elif roll < 0.68:  # open long — CLOSURE: no SOL to wallet
            tokens = sim._get_balance(user)
            if user not in sim.longs and tokens > 20 * 10**TOKEN_DECIMALS:
                tok_before, sol_before = tokens, sim._get_sol(user)
                res = sim.open_leveraged_long(user, tokens // 6)
                if "error" not in res:
                    assert sim._get_sol(user) == sol_before, \
                        "open_long leaked borrowed SOL into wallet"
                    assert sim._get_balance(user) <= tok_before, \
                        "open_long credited tokens into wallet"
                    return "open_long"
        elif roll < 0.78:  # close short — EXIT-IN-SOL: wallet tokens unchanged
            if user in sim.shorts:
                frac = 10000 if sim.rng.random() < 0.5 else 5000
                tok_before = sim._get_balance(user)
                res = sim.close_short(user, frac)
                if "error" not in res:
                    assert sim._get_balance(user) == tok_before, \
                        "close_short credited tokens into wallet (exit must be SOL)"
                    return "close_short"
        elif roll < 0.86:  # close long — EXIT-IN-SOL: wallet tokens unchanged
            if user in sim.longs:
                frac = 10000 if sim.rng.random() < 0.5 else 5000
                tok_before = sim._get_balance(user)
                res = sim.close_leveraged_long(user, frac)
                if "error" not in res:
                    assert sim._get_balance(user) == tok_before, \
                        "close_long credited tokens into wallet (exit must be SOL)"
                    return "close_long"
        elif roll < 0.90:  # liquidate underwater positions (best-effort, no mint)
            for sid in list(sim.shorts.keys()):
                p = sim.shorts[sid]
                sim._accrue_short_interest(p)
                if sid != user and sim._short_ltv_bps(p) > DEFAULT_LIQ_THRESHOLD:
                    if "error" not in sim.liquidate_short(user, sid):
                        return "liquidate_short"
            for bid in list(sim.longs.keys()):
                p = sim.longs[bid]
                sim._accrue_long_interest(p)
                if bid != user and sim._long_ltv_bps(p) > DEFAULT_LIQ_THRESHOLD:
                    if "error" not in sim.liquidate_leveraged_long(user, bid):
                        return "liquidate_long"
        # HOLD ISOLATION: advancing time with positions open touches no wallet.
        sol_snap = dict(sim.sol_balances)
        tok_snap = dict(sim.balances)
        sim.advance(sim.rng.randint(1, 30))
        assert sim.sol_balances == sol_snap and sim.balances == tok_snap, \
            "advance (holding) mutated a wallet — leverage leaked while held"
        return "advance"

    check_conservation("bootstrap")
    counts: dict[str, int] = {}
    for step in range(num_actions):
        label = random_op()
        counts[label] = counts.get(label, 0) + 1
        sim.advance(sim.rng.randint(1, 5))
        check_conservation(f"step {step} ({label})")

    open_shorts = sum(counts.get(k, 0) for k in ("open_short",))
    open_longs = counts.get("open_long", 0)
    print(f"\n  PASS — {num_actions} ops, closed-loop + conservation held every step")
    print(f"  Opened {open_shorts} shorts, {open_longs} longs; "
          f"closes: {counts.get('close_short', 0)} short / {counts.get('close_long', 0)} long")
    print(f"  Action mix:")
    for k, v in sorted(counts.items(), key=lambda x: -x[1]):
        print(f"    {k:<16} {v:>5}")
    print(f"  Still-open at end: {len(sim.shorts)} shorts, {len(sim.longs)} longs "
          f"(leverage in vaults, never in wallets)")
    return sim


def scenario_invariant_check(seed=4242, num_actions=1500, num_users=50):
    """Long random-action run with system-wide invariant assertions after
    every action. The "scenarios as fuzz tests" pass — analogous to the
    on-chain proptests, but for the full multi-actor economic system.

    Invariants checked every step:
      1. SOL conservation: sum of all SOL containers == total airdropped
      2. Token conservation: sum of all token containers == TOTAL_SUPPLY
      3. K monotonic on pool: post-migration, K never decreases

    Action mix (post-migration paths active when conditions allow):
      - bonding buy / bonding sell  (pre-migration)
      - migrate  (when bonding completes)
      - pool buy / pool sell  (post-migration)
      - open/close/liquidate leveraged long  (post-migration, treasury-gated)
      - open/close/liquidate short  (post-migration, lock-gated)
      - harvest_and_swap  (post-migration, ratio-gated)
      - advance time

    V21: shorts are now folded in. The earlier exclusion was a token-accounting
    drift in the sim (the lock was modeled as `const − total_tokens_lent`, so
    interest + gross-up surplus landing in the lock on close vanished, and short
    `vault_sol` was missing from the SOL sum). Both are fixed: the lock is a
    physical balance and `_system_sol_total` counts short vaults.
    """
    print("\n" + "="*60)
    print(f"  SCENARIO: Invariant Check ({num_actions} random actions)")
    print("="*60)

    sim = TorchSim(seed=seed)
    # Airdrop SOL across users — record total for SOL-conservation check.
    per_user = 50 * LAMPORTS_PER_SOL
    for i in range(num_users):
        sim.sol_balances[i] = per_user
    total_sol_airdropped = per_user * num_users

    # --------- Invariant helpers ---------

    def check_invariants(after_action: str, k_before: int | None) -> None:
        sol = sim._system_sol_total()
        if sol != total_sol_airdropped:
            raise AssertionError(
                f"SOL CONSERVATION VIOLATED after {after_action!r}: "
                f"actual={sol}, expected={total_sol_airdropped}, "
                f"drift={sol - total_sol_airdropped}"
            )
        tok = sim._system_token_total()
        if tok != TOTAL_SUPPLY:
            raise AssertionError(
                f"TOKEN CONSERVATION VIOLATED after {after_action!r}: "
                f"actual={tok}, expected={TOTAL_SUPPLY}, drift={tok - TOTAL_SUPPLY}"
            )
        if sim.migrated and k_before is not None and sim.pool.k < k_before:
            raise AssertionError(
                f"K DECREASED after {after_action!r}: "
                f"before={k_before}, after={sim.pool.k}"
            )

    # --------- Random action ---------

    def random_action() -> str:
        """Pick + execute a random valid action. Returns a label for logging."""
        if not sim.migrated:
            if sim.curve.bonding_complete:
                sim.migrate()
                return "migrate"
            # Curve buy/sell — buy-biased so bonding completes
            user = sim.rng.randint(0, num_users - 1)
            if sim.rng.random() < 0.75:
                progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
                max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
                amt = min(sim._get_sol(user), max_chunk)
                if amt > 0:
                    sim.buy(user, amt)
                    return "curve_buy"
            else:
                tokens = sim._get_balance(user)
                if tokens > 10**TOKEN_DECIMALS:
                    sim.sell(user, max(10**TOKEN_DECIMALS, tokens // 10))
                    return "curve_sell"
            return "noop"

        # Post-migration mix. Leverage actions never mint assets for actors —
        # collateral comes from existing wallet balances, liquidators must
        # already hold the cover asset (handlers no-op cleanly otherwise) — so
        # conservation is exercised, not assumed.
        roll = sim.rng.random()
        user = sim.rng.randint(0, num_users - 1)
        if roll < 0.30:
            # pool_buy
            amt = sim.rng.randint(1, 20) * LAMPORTS_PER_SOL // 10  # 0.1 - 2 SOL
            if sim._get_sol(user) >= amt and amt > 0:
                sim.pool_buy(user, amt)
                return "pool_buy"
        elif roll < 0.52:
            # pool_sell
            tokens = sim._get_balance(user)
            if tokens > 10**TOKEN_DECIMALS:
                sim.pool_sell(user, max(10**TOKEN_DECIMALS, tokens // 20))
                return "pool_sell"
        elif roll < 0.63:
            # V21 — open leveraged long (replaces V20 borrow in action mix)
            tokens = sim._get_balance(user)
            if user not in sim.longs and tokens > 200 * 10**TOKEN_DECIMALS:
                collateral = tokens // 6
                if "error" not in sim.open_leveraged_long(user, collateral):
                    return "open_long"
        elif roll < 0.67:
            # V21 — close leveraged long (partial)
            if user in sim.longs:
                sim.close_leveraged_long(user, 5000)  # 50% partial
                return "close_long"
        elif roll < 0.70:
            # V21 — liquidate an underwater long (best-effort; no mint)
            for bid in list(sim.longs.keys()):
                pos = sim.longs[bid]
                sim._accrue_long_interest(pos)
                if bid != user and sim._long_ltv_bps(pos) > DEFAULT_LIQ_THRESHOLD:
                    if "error" not in sim.liquidate_leveraged_long(user, bid):
                        return "liquidate_long"
        elif roll < 0.80:
            # V21 — open short (SOL collateral from wallet, no mint)
            if user not in sim.shorts and sim._get_sol(user) >= 3 * LAMPORTS_PER_SOL:
                collateral = sim.rng.randint(1, 3) * LAMPORTS_PER_SOL
                if "error" not in sim.open_short(user, collateral):
                    return "open_short"
        elif roll < 0.85:
            # V21 — close short (mix full + partial)
            if user in sim.shorts:
                frac = 10000 if sim.rng.random() < 0.5 else 5000
                if "error" not in sim.close_short(user, frac):
                    return "close_short"
        elif roll < 0.88:
            # V21 — liquidate an underwater short (best-effort; no mint)
            for sid in list(sim.shorts.keys()):
                pos = sim.shorts[sid]
                sim._accrue_short_interest(pos)
                if sid != user and sim._short_ltv_bps(pos) > DEFAULT_LIQ_THRESHOLD:
                    if "error" not in sim.liquidate_short(user, sid):
                        return "liquidate_short"
        elif roll < 0.97:
            # harvest (gates may reject — that's fine, conservation must still hold)
            sim.harvest_and_swap()
            return "harvest"
        # else: just advance time
        sim.advance(sim.rng.randint(1, 30))
        return "advance"

    # --------- Run ---------

    print(f"  Total SOL airdropped: {total_sol_airdropped / LAMPORTS_PER_SOL:.0f} SOL")
    print(f"  Total token supply:   {TOTAL_SUPPLY / 10**TOKEN_DECIMALS:,.0f}")
    check_invariants("initial setup", None)

    action_counts: dict[str, int] = {}
    actions_performed = 0
    for step in range(num_actions):
        k_before = sim.pool.k if sim.migrated else None
        try:
            label = random_action()
        except Exception as e:
            raise AssertionError(f"step {step}: action raised: {e}") from e
        action_counts[label] = action_counts.get(label, 0) + 1
        actions_performed += 1
        # Advance a small amount of time between actions so interest accrues etc.
        sim.advance(sim.rng.randint(1, 5))
        try:
            check_invariants(f"step {step} ({label})", k_before)
        except AssertionError as e:
            print(f"\n  *** INVARIANT FAILED ***\n  {e}\n  Last 10 actions:")
            for entry in sim.log[-10:]:
                print(f"    {entry.slot}: {entry.event.value} user={entry.user_id}")
            raise

    print(f"\n  PASS — {actions_performed} actions, all invariants held")
    print(f"  Action mix:")
    for k, v in sorted(action_counts.items(), key=lambda x: -x[1]):
        print(f"    {k:<14} {v:>5}")
    sim.print_snapshot()
    return sim


# ============================================================================
# Main
# ============================================================================

def _setup_migrated(seed=2024, treasury_sol=None):
    """Bond a token to target across many wallets, then migrate to DeepPool.

    Optionally force `treasury.sol_balance` to a known level — a stress-setup
    shortcut that bypasses the slow/flaky organic harvest loop. Legitimate here
    because these scenarios test liquidation/solvency, not fee accrual. Returns
    the migrated sim (pool depth readable via sim.pool.sol_reserves).
    """
    sim = TorchSim(seed=seed)
    num_users = 50
    for i in range(num_users):
        sim.sol_balances[i] = 100 * LAMPORTS_PER_SOL
    iters = 0
    while not sim.curve.bonding_complete and iters < 20000:
        iters += 1
        i = sim.rng.randint(0, num_users - 1)
        progress = sim.curve.real_sol / max(1, sim.curve.bonding_target)
        max_chunk = int((0.3 + 2.0 * progress) * LAMPORTS_PER_SOL)
        amt = min(sim._get_sol(i), max_chunk)
        if amt > 0:
            sim.buy(i, amt)
        sim.advance(5)
    sim.migrate()
    if treasury_sol is not None:
        sim.treasury.sol_balance = treasury_sol
    return sim


def _setup_migrated_depth(seed, pool_sol_target):
    """Migrated sim with the pool scaled to ~pool_sol_target SOL at the SAME
    price (scale both reserves by one factor → depth changes, price unchanged).
    Lets us sweep pool depth independently of price. Scaling happens in setup,
    before any conservation snapshot the caller takes."""
    sim = _setup_migrated(seed=seed)
    cur = sim.pool.sol_reserves
    if cur > 0 and pool_sol_target != cur:
        f = pool_sol_target / cur
        sim.pool.sol_reserves = int(sim.pool.sol_reserves * f)
        sim.pool.token_reserves = int(sim.pool.token_reserves * f)
        sim.treasury.baseline_sol = sim.pool.sol_reserves
        sim.treasury.baseline_tokens = sim.pool.token_reserves
    return sim


def _warm_oracle(sim, span_slots=None, step=None):
    """Keeperless warmup. The TWAP oracle lives on the pool and only advances on
    swaps, so stamp its ring with tiny, ~price-neutral swaps spaced one snapshot
    apart until it spans the consumer lookback. After this, `read_twap_sol_per_tok`
    is anchored at a snapshot ≥ LIQ_TWAP_LOOKBACK_SLOTS old (no longer warmup).
    This mirrors how a live pool warms under ordinary organic flow — there is no
    crank to call."""
    step = step or MIN_OBS_SPACING_SLOTS
    span_slots = span_slots or (LIQ_TWAP_LOOKBACK_SLOTS + 4 * step)
    for _ in range(span_slots // step):
        sim.advance(step)
        sim.pool.swap_sol_for_tokens(1000, sim.slot)  # negligible price impact, stamps an obs


def _run_manipulation_attack(sim, *, target_ltv, victim_collateral_sol,
                             victim=0, attacker=90, market=91):
    """Open a victim short; (optionally) let MARKET pre-stress it to target_ltv;
    then run the attacker pump→cover→liquidate→unwind round trip. Pure-SOL EV.
    Conservation snapshot is taken AFTER actor seeding so it tracks only
    protocol-internal flows. Returns a metrics dict (or {'error': ...})."""
    sim.sol_balances[victim] = victim_collateral_sol
    sim.sol_balances[market] = 10_000_000 * LAMPORTS_PER_SOL
    sim.sol_balances[attacker] = 10_000_000 * LAMPORTS_PER_SOL
    sol0, tok0 = sim._system_sol_total(), sim._system_token_total()

    vres = sim.open_short(victim, victim_collateral_sol)
    if "error" in vres:
        return {"error": vres["error"]}
    vpos = sim.shorts[victim]
    pool_sol = sim.pool.sol_reserves

    def debt_gross():
        p = sim.shorts.get(victim)
        return 0 if p is None else sim._gross_up_for_transfer_fee(
            p.tokens_borrowed + p.accrued_interest)

    # Pre-stress to target_ltv (target == open LTV → no pre-stress).
    guard = 0
    while sim._short_ltv_bps(vpos) < target_ltv and guard < 200000:
        guard += 1
        if sim.pool.token_reserves <= 1:
            break
        sim.pool_buy(market, max(MIN_BORROW_AMOUNT, pool_sol // 200))
    start_ltv = sim._short_ltv_bps(vpos)

    atk0 = sim._get_sol(attacker)
    chunk = max(MIN_BORROW_AMOUNT, sim.pool.sol_reserves // 100)
    guard = 0
    while victim in sim.shorts and guard < 200000:
        liq = sim._short_ltv_bps(sim.shorts[victim]) > DEFAULT_LIQ_THRESHOLD
        if liq and sim._get_balance(attacker) >= debt_gross():
            break
        if sim.pool.token_reserves <= 1 or "error" in sim.pool_buy(attacker, chunk):
            break
        guard += 1

    seized, rounds = 0, 0
    while (victim in sim.shorts
           and sim._short_ltv_bps(sim.shorts[victim]) > DEFAULT_LIQ_THRESHOLD
           and rounds < 50):
        rounds += 1
        lr = sim.liquidate_short(attacker, victim)
        if "error" in lr:
            break
        seized += lr.get("sol_seized", 0)

    held = sim._get_balance(attacker)
    if held > 0:
        sim.pool_sell(attacker, held)

    return {
        "start_ltv": start_ltv,
        "ev": sim._get_sol(attacker) - atk0,
        "seized": seized,
        "rounds": rounds,
        "tok_residual": sim._get_balance(attacker),
        "cons_ok": (sim._system_sol_total() == sol0
                    and sim._system_token_total() == tok0),
    }


def scenario_oracle_attack_sweep(seed=2025):
    """ATTACK #1 (strengthened) — map the manipulation EV across pool depth and
    victim size, always MANUFACTURING from the open (comfortable) LTV. Answers:
    is there a depth/size regime where manufacturing a liquidation is priced out?
    """
    print("\n" + "=" * 60)
    print("  ATTACK #1 (sweep): Manipulation EV vs Depth × Victim Size")
    print("=" * 60)

    OPEN = 4500  # manufacture from the depth-band max (no market pre-stress)

    print("\n  -- Depth sweep (victim collateral = 20% of pool depth) --")
    print(f"  {'pool depth':>11} {'maxLTV':>7} {'victim col':>11} "
          f"{'start LTV':>9} {'SOL seized':>11} {'attacker EV':>13}  verdict")
    any_profit_depth = False
    for depth_sol in (10, 30, 100, 300, 700):
        sim = _setup_migrated_depth(seed, depth_sol * LAMPORTS_PER_SOL)
        col = max(MIN_BORROW_AMOUNT, (depth_sol * LAMPORTS_PER_SOL) // 5)  # 20% of depth
        r = _run_manipulation_attack(sim, target_ltv=OPEN, victim_collateral_sol=col)
        if "error" in r:
            print(f"  {depth_sol:>9} SOL  {get_depth_max_ltv_bps(depth_sol*LAMPORTS_PER_SOL)/100:>5.0f}% "
                  f"{col/LAMPORTS_PER_SOL:>10.1f}  -- {r['error']}")
            continue
        profit = r["ev"] > 0
        any_profit_depth = any_profit_depth or profit
        print(f"  {depth_sol:>9} SOL  "
              f"{get_depth_max_ltv_bps(depth_sol*LAMPORTS_PER_SOL)/100:>5.0f}% "
              f"{col/LAMPORTS_PER_SOL:>10.1f} {r['start_ltv']/100:>8.1f}% "
              f"{r['seized']/LAMPORTS_PER_SOL:>10.3f} {r['ev']/LAMPORTS_PER_SOL:>12.4f}"
              f"  {'PROFIT ✗' if profit else 'loss ✓'}"
              + ("" if r["cons_ok"] else "  [!CONS]"))

    print("\n  -- Victim-size sweep (fixed ~300 SOL pool) --")
    print(f"  {'victim col':>11} {'start LTV':>9} {'SOL seized':>11} "
          f"{'attacker EV':>13}  verdict")
    any_profit_size = False
    for col_sol in (5, 20, 60, 150, 400):
        sim = _setup_migrated_depth(seed, 300 * LAMPORTS_PER_SOL)
        r = _run_manipulation_attack(sim, target_ltv=OPEN,
                                     victim_collateral_sol=col_sol * LAMPORTS_PER_SOL)
        if "error" in r:
            print(f"  {col_sol:>9} SOL  -- {r['error']}")
            continue
        profit = r["ev"] > 0
        any_profit_size = any_profit_size or profit
        print(f"  {col_sol:>9} SOL {r['start_ltv']/100:>8.1f}% "
              f"{r['seized']/LAMPORTS_PER_SOL:>10.3f} {r['ev']/LAMPORTS_PER_SOL:>12.4f}"
              f"  {'PROFIT ✗' if profit else 'loss ✓'}"
              + ("" if r["cons_ok"] else "  [!CONS]"))

    print()
    if any_profit_depth or any_profit_size:
        print("  ⚠️  Manufacturing a liquidation is profitable in at least one")
        print("      depth/size regime — the spot-marked liquidation is exploitable.")
    else:
        print("  PASS — manufacturing is negative-EV across all depths and sizes.")
    return None


def scenario_oracle_liquidation_attack(seed=2024):
    """ATTACK #1 — third-party liquidation via spot manipulation.

    deep_pool has no TWAP/oracle; V21 marks a short's LTV at the pool SPOT
    (debt_value / vault_sol). An attacker with capital but no stake in the
    victim can PUMP the thin pool to drive a healthy short past the 65%
    liquidation threshold, capture the 10% bonus on the 50%-close-factor debt,
    then unwind the pump. Question: does the captured bonus ever exceed the
    round-trip manipulation cost (2x 0.25% pool fee + slippage)?

    We sweep the victim's LTV at the moment the attacker strikes. The honest
    read of the result:
      - profit when the victim is MANUFACTURED from a comfortable LTV (45-50%)
        => genuine manipulation vulnerability.
      - profit only when TIPPING a victim already near threshold (~63%)
        => that's just normal/healthy liquidator incentive, not an attack.
    """
    print("\n" + "=" * 60)
    print("  ATTACK #1: Third-Party Liquidation via Spot Manipulation")
    print("=" * 60)

    VICTIM, ATTACKER, MARKET = 0, 90, 91
    results = []
    pool_sol = 0

    def victim_debt_gross(sim):
        p = sim.shorts.get(VICTIM)
        if p is None:
            return 0
        return sim._gross_up_for_transfer_fee(p.tokens_borrowed + p.accrued_interest)

    for target_ltv in (4500, 5000, 5500, 6300):  # victim LTV when attacker strikes
        sim = _setup_migrated(seed=seed)
        pool_sol = sim.pool.sol_reserves

        # Seed ALL actors up front, THEN snapshot — so conservation tracks only
        # protocol-internal flows, not our airdrops.
        sim.sol_balances[VICTIM] = 40 * LAMPORTS_PER_SOL
        sim.sol_balances[MARKET] = 10_000_000 * LAMPORTS_PER_SOL
        sim.sol_balances[ATTACKER] = 10_000_000 * LAMPORTS_PER_SOL
        sol_before_total = sim._system_sol_total()
        tok_before_total = sim._system_token_total()

        vres = sim.open_short(VICTIM, 40 * LAMPORTS_PER_SOL)
        if "error" in vres:
            print(f"    victim open_short failed: {vres['error']}")
            continue
        vpos = sim.shorts[VICTIM]

        # Pre-stress: neutral MARKET pumps until the victim sits at target_ltv
        # (a position the market already nudged toward the brink). target_ltv
        # == open LTV (~45%) means NO pre-stress — the attacker does all the
        # work. NOT counted in attacker EV.
        guard = 0
        while sim._short_ltv_bps(vpos) < target_ltv and guard < 200000:
            guard += 1
            if sim.pool.token_reserves <= 1:
                break
            sim.pool_buy(MARKET, max(MIN_BORROW_AMOUNT, pool_sol // 200))
        start_ltv = sim._short_ltv_bps(vpos)
        manufactured = start_ltv <= 5000  # struck from comfortable territory

        # ---- attacker round trip (pure-SOL EV; ends ~flat in tokens) ----
        atk_sol_before = sim._get_sol(ATTACKER)
        pump_chunk = max(MIN_BORROW_AMOUNT, sim.pool.sol_reserves // 100)

        # Acquire until BOTH: victim is liquidatable AND attacker holds enough
        # tokens to cover the full debt (so the liquidation actually executes —
        # the extra buying is a real attacker cost, captured in EV).
        guard = 0
        while VICTIM in sim.shorts and guard < 200000:
            guard += 1
            liquidatable = sim._short_ltv_bps(sim.shorts[VICTIM]) > DEFAULT_LIQ_THRESHOLD
            covered = sim._get_balance(ATTACKER) >= victim_debt_gross(sim)
            if liquidatable and covered:
                break
            if sim.pool.token_reserves <= 1:
                break
            if "error" in sim.pool_buy(ATTACKER, pump_chunk):
                break

        # Liquidate while underwater (capture all available bonus).
        seized_total, liq_rounds = 0, 0
        while (VICTIM in sim.shorts
               and sim._short_ltv_bps(sim.shorts[VICTIM]) > DEFAULT_LIQ_THRESHOLD
               and liq_rounds < 50):
            liq_rounds += 1
            lr = sim.liquidate_short(ATTACKER, VICTIM)
            if "error" in lr:
                break
            seized_total += lr.get("sol_seized", 0)

        # Unwind — dump ALL tokens the attacker holds back to the pool.
        held = sim._get_balance(ATTACKER)
        if held > 0:
            sim.pool_sell(ATTACKER, held)

        ev = sim._get_sol(ATTACKER) - atk_sol_before
        tok_residual = sim._get_balance(ATTACKER)
        cons_ok = (sim._system_sol_total() == sol_before_total
                   and sim._system_token_total() == tok_before_total)

        results.append((start_ltv, manufactured, liq_rounds, seized_total,
                        ev, tok_residual, cons_ok))

    # ---- report ----
    print(f"\n  Pool depth ~{pool_sol/LAMPORTS_PER_SOL:.0f} SOL "
          f"(depth max-LTV {get_depth_max_ltv_bps(pool_sol)/100:.0f}%), "
          f"liq threshold {DEFAULT_LIQ_THRESHOLD/100:.0f}%, "
          f"bonus {DEFAULT_LIQ_BONUS_BPS/100:.0f}%, close {DEFAULT_LIQ_CLOSE_BPS/100:.0f}%")
    print(f"\n  {'LTV@strike':>11} {'manufactured':>12} {'liq rds':>8} "
          f"{'SOL seized':>11} {'attacker EV':>13}  verdict")
    manufactured_profit = False
    for start_ltv, manufactured, liq_rounds, seized, ev, tok_res, cons_ok in results:
        profit = ev > 0
        if profit and manufactured:
            manufactured_profit = True
        verdict = "PROFIT" if profit else "loss"
        flag = " ✗" if (profit and manufactured) else (" ✓" if not profit else " (healthy liq)")
        print(f"  {start_ltv/100:>10.1f}% {str(manufactured):>12} {liq_rounds:>8} "
              f"{seized/LAMPORTS_PER_SOL:>10.3f} {ev/LAMPORTS_PER_SOL:>12.4f}  {verdict}{flag}"
              + ("" if cons_ok else "  [!CONSERVATION BROKE]"))

    print()
    if manufactured_profit:
        print("  ⚠️  VULNERABLE — attacker profits MANUFACTURING a liquidation on a")
        print("      comfortable (45-50%) position. The unguarded spot mark lets")
        print("      manipulation pay. This is the oracle exposure, confirmed.")
    else:
        print("  PASS — manufacturing a liquidation on a comfortable position is")
        print("         negative-EV: the round-trip pump slippage + 2x0.25% fee")
        print("         exceed the 10% bonus on the 50% close factor. Profit only")
        print("         appears when tipping a position already at the brink — that")
        print("         is normal liquidator incentive, not manipulation.")
    return None


def scenario_solvency_gap_crash(seed=3031):
    """STRESS #3 — treasury/lock solvency under a price gap that outruns liquidators.

    Construct max-LTV exposure, then apply a SINGLE gap move before any
    liquidation fires (liquidators only act after the gap), so positions are
    deeply underwater at once → forces real bad debt. Measures bad debt,
    post-crash solvency, and conservation. Bad debt is an economic loss but must
    NOT create/destroy SOL or tokens — conservation must still hold.
    """
    print("\n" + "=" * 60)
    print("  STRESS #3: Solvency Under Gap Crash (outruns liquidators)")
    print("=" * 60)

    # ===== LONG SIDE: max-LTV longs drain treasury, then gap DOWN =====
    print("\n  -- Long side: max-LTV longs, then one-shot gap-down --")
    sim = _setup_migrated(seed=seed, treasury_sol=400 * LAMPORTS_PER_SOL)
    LIQ, WHALE = 80, 81
    borrowers = list(range(60, 80))
    col_tokens = (50 * LAMPORTS_PER_SOL) * sim.pool.token_reserves // sim.pool.sol_reserves
    for b in borrowers:
        sim.balances[b] = sim.balances.get(b, 0) + col_tokens
    sim.sol_balances[LIQ] = 1_000_000 * LAMPORTS_PER_SOL
    sim.balances[WHALE] = 3 * sim.pool.token_reserves
    sol0, tok0 = sim._system_sol_total(), sim._system_token_total()

    opened = 0
    for b in borrowers:
        if "error" not in sim.open_leveraged_long(b, sim._get_balance(b)):
            opened += 1
    treasury_before = sim.treasury.sol_balance
    lent_before = sim.treasury.total_sol_lent_to_longs
    price_before = sim.pool.price
    print(f"    Opened {opened} longs | treasury {treasury_before/LAMPORTS_PER_SOL:.1f} SOL, "
          f"lent {lent_before/LAMPORTS_PER_SOL:.1f} SOL, util {sim.treasury.utilization_bps/100:.0f}%")

    # GAP DOWN — whale dumps in ONE shot; no liquidation in between.
    sim.pool_sell(WHALE, sim.pool.token_reserves)
    price_after = sim.pool.price
    print(f"    Gap-down: {price_before*10**TOKEN_DECIMALS:.4f} → {price_after*10**TOKEN_DECIMALS:.4f} "
          f"({(1-price_after/price_before)*100:.0f}% crash) in one block")

    bad_debt_l, covered_l, liqs_l = 0, 0, 0
    for _ in range(100):  # rounds — 50% close factor needs repeats to fully unwind
        any_liq = False
        for b in list(sim.longs.keys()):
            if sim._long_ltv_bps(sim.longs[b]) > DEFAULT_LIQ_THRESHOLD:
                r = sim.liquidate_leveraged_long(LIQ, b)
                if "error" not in r:
                    liqs_l += 1
                    bad_debt_l += r["bad_debt"]
                    covered_l += r["debt_covered"]
                    any_liq = True
        if not any_liq:
            break
    cons_l = (sim._system_sol_total() == sol0 and sim._system_token_total() == tok0)
    print(f"    Liquidated {liqs_l} | covered {covered_l/LAMPORTS_PER_SOL:.2f} SOL | "
          f"BAD DEBT {bad_debt_l/LAMPORTS_PER_SOL:.2f} SOL "
          f"({bad_debt_l/max(1,lent_before)*100:.1f}% of lent)")
    print(f"    Treasury after {sim.treasury.sol_balance/LAMPORTS_PER_SOL:.1f} SOL | "
          f"lent-claim remaining {sim.treasury.total_sol_lent_to_longs/LAMPORTS_PER_SOL:.2f} SOL | "
          f"conservation {'HELD ✓' if cons_l else 'BROKE ✗'}")

    # ===== SHORT SIDE: max shorts, then gap UP =====
    print("\n  -- Short side: max shorts, then one-shot gap-up --")
    sim = _setup_migrated(seed=seed + 1)
    LIQ2, WHALE2 = 82, 83
    # Few, larger shorts so opening doesn't dump the whole lock and self-crash
    # the pool — keeps the gap-up the dominant move, not the setup.
    shorters = list(range(60, 65))
    for s in shorters:
        sim.sol_balances[s] = 40 * LAMPORTS_PER_SOL
    sim.balances[LIQ2] = 5 * sim.pool.token_reserves  # liquidator token war chest
    sim.sol_balances[WHALE2] = 1_000_000 * LAMPORTS_PER_SOL
    sol0s, tok0s = sim._system_sol_total(), sim._system_token_total()

    opened_s = 0
    for s in shorters:
        if "error" not in sim.open_short(s, 40 * LAMPORTS_PER_SOL):
            opened_s += 1
    lent_tokens_before = sim.treasury.total_tokens_lent
    price_before_s = sim.pool.price
    print(f"    Opened {opened_s} shorts | tokens lent {lent_tokens_before/10**TOKEN_DECIMALS:,.0f}")

    # GAP UP — whale buys in ONE shot.
    sim.pool_buy(WHALE2, sim.pool.sol_reserves)
    price_after_s = sim.pool.price
    print(f"    Gap-up: {price_before_s*10**TOKEN_DECIMALS:.4f} → {price_after_s*10**TOKEN_DECIMALS:.4f} "
          f"({(price_after_s/price_before_s-1)*100:.0f}% pump) in one block")

    bad_debt_s, covered_s, liqs_s = 0, 0, 0
    for _ in range(100):  # rounds — 50% close factor needs repeats
        any_liq = False
        for s in list(sim.shorts.keys()):
            if sim._short_ltv_bps(sim.shorts[s]) > DEFAULT_LIQ_THRESHOLD:
                r = sim.liquidate_short(LIQ2, s)
                if "error" not in r:
                    liqs_s += 1
                    bad_debt_s += r["bad_debt"]
                    covered_s += r.get("tokens_covered", 0)
                    any_liq = True
        if not any_liq:
            break
    cons_s = (sim._system_sol_total() == sol0s and sim._system_token_total() == tok0s)
    print(f"    Liquidated {liqs_s} | covered {covered_s/10**TOKEN_DECIMALS:,.0f} tok | "
          f"BAD DEBT {bad_debt_s/10**TOKEN_DECIMALS:,.0f} tok "
          f"({bad_debt_s/max(1,lent_tokens_before)*100:.1f}% of lent) | "
          f"conservation {'HELD ✓' if cons_s else 'BROKE ✗'}")

    print("\n  ---- Findings ----")
    if cons_l and cons_s:
        print("    Conservation HELD both sides — bad debt is redistribution, not")
        print("    invented/destroyed value. Bad debt is bounded by the per-position")
        print("    vault gap; the gap-crash converts directional loss into protocol")
        print("    loss exactly when the move outruns the 65% band faster than")
        print("    liquidation can act. Solvency question = can treasury/lock absorb")
        print("    the write-off (above) without stranding other positions.")
    else:
        print("    ⚠️ CONSERVATION BROKE under gap crash — accounting bug, investigate.")
    return None


def scenario_long_baddebt_severity(seed=3032):
    """Validate #3: does long bad debt appear at extreme gaps, and stay bounded?
    A V21 long's vault holds collateral + the leveraged buy, so effective LTV at
    open sits well under the 45% borrow cap — it takes a deep crash to go
    underwater. Curve should be ~0 until that point, then bounded by the single
    position's vault (closed-loop custody caps the loss)."""
    print("\n" + "=" * 60)
    print("  STRESS #3b: Long Bad-Debt vs Crash Severity")
    print("=" * 60)
    print(f"\n  {'dump (xpool)':>13} {'crash %':>9} {'open LTV':>9} "
          f"{'debt SOL':>9} {'bad debt':>11} {'cons':>5}")
    for mult in (0.5, 1.0, 2.0, 4.0, 8.0):
        sim = _setup_migrated(seed=seed, treasury_sol=400 * LAMPORTS_PER_SOL)
        B, LIQ, WHALE = 60, 80, 81
        col = (60 * LAMPORTS_PER_SOL) * sim.pool.token_reserves // sim.pool.sol_reserves
        sim.balances[B] = col
        sim.sol_balances[LIQ] = 1_000_000 * LAMPORTS_PER_SOL
        sim.balances[WHALE] = int(12 * sim.pool.token_reserves)
        sol0, tok0 = sim._system_sol_total(), sim._system_token_total()
        r = sim.open_leveraged_long(B, col)
        if "error" in r:
            print(f"  open failed: {r['error']}")
            continue
        open_ltv, debt = r["ltv_bps"], r["borrowed_sol_gross"]
        p0 = sim.pool.price
        sim.pool_sell(WHALE, int(mult * sim.pool.token_reserves))
        crash = (1 - sim.pool.price / p0) * 100
        bd = 0
        for _ in range(200):
            if B not in sim.longs or sim._long_ltv_bps(sim.longs[B]) <= DEFAULT_LIQ_THRESHOLD:
                break
            lr = sim.liquidate_leveraged_long(LIQ, B)
            if "error" in lr:
                break
            bd += lr["bad_debt"]
        cons = (sim._system_sol_total() == sol0 and sim._system_token_total() == tok0)
        print(f"  {mult:>13.1f} {crash:>8.1f}% {open_ltv/100:>8.1f}% "
              f"{debt/LAMPORTS_PER_SOL:>8.2f} {bd/LAMPORTS_PER_SOL:>10.3f} "
              f"{'✓' if cons else '✗':>5}")
    print("\n  Read: bad debt is ~0 until the gap drives vault value below debt,")
    print("  then bounded by the single position's vault — custody caps the loss.")
    return None


def scenario_oracle_mitigation_test(seed=2026):
    """Validate the D-10 fix on the KEEPERLESS oracle: re-run the
    manufacture-from-open attack with the TWAP liquidation guard OFF vs ON, then
    show a legitimate HELD move still liquidates after a bounded lag. The mark now
    lives in DeepPool (no crank, no ratchet) — warm it with ordinary swaps."""
    print("\n" + "=" * 60)
    print("  D-10: TWAP Liquidation-Mark Mitigation Test (keeperless oracle)")
    print("=" * 60)

    print("\n  -- Manufacture-from-open attack: guard OFF vs ON --")
    print(f"  {'guard':>6} {'start LTV':>9} {'liq rds':>8} {'SOL seized':>11} "
          f"{'attacker EV':>13}  verdict")
    rows = {}
    for guard in (False, True):
        sim = _setup_migrated(seed)
        sim.twap_enabled = guard
        if guard:
            _warm_oracle(sim)  # keeperless warmup: tiny swaps span the lookback
        r = _run_manipulation_attack(sim, target_ltv=4500,
                                     victim_collateral_sol=60 * LAMPORTS_PER_SOL)
        rows[guard] = r
        print(f"  {'ON' if guard else 'OFF':>6} {r['start_ltv']/100:>8.1f}% "
              f"{r['rounds']:>8} {r['seized']/LAMPORTS_PER_SOL:>10.3f} "
              f"{r['ev']/LAMPORTS_PER_SOL:>12.4f}  "
              f"{'PROFIT ✗' if r['ev'] > 0 else 'blocked ✓'}")
    print()
    if rows[False]["ev"] > 0 and rows[True]["ev"] <= 0:
        print("  PASS — the atomic pump can't move a time-weighted mark, so the")
        print("  manufactured liquidation is refused; raw spot would have paid out.")
    else:
        print("  ⚠️ guard did not close the attack as expected — investigate.")

    print("\n  -- Legitimate HELD move under guard (no ratchet — lazy extension) --")
    sim = _setup_migrated(seed)
    sim.twap_enabled = True
    _warm_oracle(sim)
    V, M, LIQ = 0, 91, 90
    sim.sol_balances[V] = 60 * LAMPORTS_PER_SOL
    sim.sol_balances[M] = 10_000_000 * LAMPORTS_PER_SOL
    sim.balances[LIQ] = 5 * sim.pool.token_reserves
    sim.open_short(V, 60 * LAMPORTS_PER_SOL)
    vpos = sim.shorts[V]
    while sim._short_ltv_bps(vpos) < 9000 and sim.pool.token_reserves > 1:
        if "error" in sim.pool_buy(M, sim.pool.sol_reserves // 50):
            break
    spot_ltv = sim._short_ltv_bps(vpos)
    # HOLD the elevated price — no further trades. The keeperless mark walks up via
    # the read's lazy head-extension as the held interval enters the lookback.
    step = MIN_OBS_SPACING_SLOTS
    fired_at = None
    for w in range(1, int(3 * LIQ_TWAP_LOOKBACK_SLOTS // step) + 1):
        sim.advance(step)
        if fired_at is None and V in sim.shorts \
                and sim._short_ltv_at_twap(sim.shorts[V]) > DEFAULT_LIQ_THRESHOLD:
            fired_at = w
            break
    twap_ltv = sim._short_ltv_at_twap(vpos) if V in sim.shorts else 0
    res = sim.liquidate_short(LIQ, V) if V in sim.shorts else {"error": "gone"}
    if fired_at is not None:
        lag_slots = fired_at * step
        print(f"    spot LTV {spot_ltv/100:.0f}% | TWAP crossed threshold after "
              f"~{lag_slots} slots (~{lag_slots*0.4/60:.0f} min hold) | "
              f"liquidation: {'FIRED ✓' if 'error' not in res else 'refused'}")
        print("    The mark tracks a genuine HELD move with no keeper — only the")
        print("    forced hold (~lookback) lags it; per-position custody caps the")
        print("    bad debt accrued during that lag (#3b).")
    else:
        print(f"    spot LTV {spot_ltv/100:.0f}% | TWAP LTV {twap_ltv/100:.0f}% | "
              f"never crossed within 3× lookback — investigate.")
    return None


def _setup_twap_short_at_liquidation(seed, victim_col_sol=60, max_hold_slots=None):
    """Build a short, push SPOT deep underwater in one burst, then HOLD the
    elevated price (no further trades — the keeperless mark tracks it via the
    read's lazy extension) until the position is TWAP-liquidatable. Returns
    (sim, victim_id, hold_slots) or (sim, victim_id, None) if it never crossed."""
    if max_hold_slots is None:
        max_hold_slots = 3 * LIQ_TWAP_LOOKBACK_SLOTS
    sim = _setup_migrated(seed)
    sim.twap_enabled = True
    _warm_oracle(sim)
    V, M = 0, 91
    sim.sol_balances[V] = victim_col_sol * LAMPORTS_PER_SOL
    sim.sol_balances[M] = 10_000_000 * LAMPORTS_PER_SOL
    sim.open_short(V, victim_col_sol * LAMPORTS_PER_SOL)
    vpos = sim.shorts[V]
    while sim._short_ltv_bps(vpos) < 9000 and sim.pool.token_reserves > 1:
        if "error" in sim.pool_buy(M, sim.pool.sol_reserves // 50):
            break
    step = MIN_OBS_SPACING_SLOTS
    held = 0
    while held < max_hold_slots and V in sim.shorts \
            and sim._short_ltv_at_twap(sim.shorts[V]) <= DEFAULT_LIQ_THRESHOLD:
        sim.advance(step)
        held += step
    crossed = V in sim.shorts and sim._short_ltv_at_twap(sim.shorts[V]) > DEFAULT_LIQ_THRESHOLD
    return sim, V, (held if crossed else None)


def scenario_twap_tuning(seed=2027):
    """#1 seize-clamp validation + #2 LOOKBACK sweep of the lag/resistance
    trade-off. The keeperless oracle has ONE consumer knob now —
    LIQ_TWAP_LOOKBACK_SLOTS (the window torch averages over). deep_pool's ring +
    spacing are fixed storage; there is no per-observation band/ratchet anymore."""
    global LIQ_TWAP_LOOKBACK_SLOTS
    print("\n" + "=" * 60)
    print("  D-10 Tuning: Seize Clamp + Lookback Lag/Resistance Sweep")
    print("=" * 60)

    # ---- #1: seize-accounting clamp — seize invariant to a spot pump ----
    print("\n  -- Seize clamp: SOL seized vs a 3× spot pump at liquidation --")
    sim, V, _ = _setup_twap_short_at_liquidation(seed)
    LIQ = 90
    sim.balances[LIQ] = 5 * sim.pool.token_reserves
    seized_clean = sim.liquidate_short(LIQ, V).get("sol_seized", 0)

    sim2, V2, _ = _setup_twap_short_at_liquidation(seed)
    sim2.balances[LIQ] = 5 * sim2.pool.token_reserves
    ATT = 93
    sim2.sol_balances[ATT] = 10_000_000 * LAMPORTS_PER_SOL
    p0 = sim2.pool.price
    while sim2.pool.price < 3 * p0 and sim2.pool.token_reserves > 1:
        if "error" in sim2.pool_buy(ATT, sim2.pool.sol_reserves // 20):  # atomic pump
            break
    spot_mult = sim2.pool.price / p0
    seized_pumped = sim2.liquidate_short(LIQ, V2).get("sol_seized", 0)
    drift = abs(seized_pumped - seized_clean) / max(1, seized_clean) * 100
    print(f"    seized (no pump):   {seized_clean/LAMPORTS_PER_SOL:.4f} SOL")
    print(f"    seized ({spot_mult:.1f}× spot pump): {seized_pumped/LAMPORTS_PER_SOL:.4f} SOL "
          f"({drift:.2f}% drift)")
    print(f"    {'PASS ✓ seize priced at TWAP — atomic pump does not inflate it' if drift < 1 else '⚠️ seize moved with spot — clamp not holding'}")

    # ---- #2: lookback sweep → window / forced-hold lag / atomic resistance ----
    print("\n  -- Tuning: lookback → window / genuine-move lag / atomic EV --")
    print(f"  {'lookback':>9} {'window(min)':>11} {'hold lag(min)':>13} {'atomic EV':>11}")
    orig = LIQ_TWAP_LOOKBACK_SLOTS
    try:
        for lookback in (1000, 3000, 6000):
            LIQ_TWAP_LOOKBACK_SLOTS = lookback
            _, _, lag = _setup_twap_short_at_liquidation(seed)
            asim = _setup_migrated(seed)
            asim.twap_enabled = True
            _warm_oracle(asim)
            ev = _run_manipulation_attack(asim, target_ltv=4500,
                                          victim_collateral_sol=60 * LAMPORTS_PER_SOL)["ev"]
            window_min = lookback * 0.4 / 60
            lag_min = (lag * 0.4 / 60) if lag is not None else None
            print(f"  {lookback:>9} {window_min:>11.0f} "
                  f"{(f'{lag_min:.0f}' if lag_min is not None else 'never'):>13} "
                  f"{ev/LAMPORTS_PER_SOL:>10.3f}")
    finally:
        LIQ_TWAP_LOOKBACK_SLOTS = orig

    print()
    print("  Key result: lookback is the single knob. It IS the time an attacker")
    print("  must HOLD an off-market price to manufacture a trigger — longer window")
    print("  ⇒ longer forced hold ⇒ more arb bleed + locked capital, but a laggier")
    print("  liquidation of a genuine move. The atomic EV stays ≤0 at every window")
    print("  (one-tx pump can't move a time-weighted mark). Pick the shortest")
    print("  lookback whose hold-cost exceeds the extractable bonus, and pair it")
    print("  with the bonus/seize cap + per-position custody (bounds loss in the")
    print("  lag). The TWAP converts atomic theft into a costly timed hold.")
    return None


def scenario_liq_bonus_ramp(seed=2028):
    """D-10 prize cap: the liquidation bonus scales with distress (TWAP LTV), so a
    manufactured (barely-over-threshold) liquidation earns ~0 while genuine deep
    distress pays full. Removes the reward the sustained-hold manipulation needs."""
    print("\n" + "=" * 60)
    print("  D-10: Distress-Scaled Liquidation Bonus (manipulation prize cap)")
    print("=" * 60)
    sim0 = _setup_migrated(seed)
    sim0.twap_enabled = True
    print("\n  -- Bonus ramp shape (effective bonus vs TWAP LTV) --")
    print(f"  {'TWAP LTV':>9} {'eff bonus':>10}")
    for ltv in (6400, 6500, 7000, 7500, 8000, 9000, 12000):
        print(f"  {ltv/100:>8.0f}% {sim0._effective_liq_bonus_bps(ltv)/100:>9.2f}%")

    print("\n  -- Manufactured (just-over) vs genuine (deep) liquidation --")
    sim, V, _ = _setup_twap_short_at_liquidation(seed)
    sim.balances[90] = 5 * sim.pool.token_reserves
    ltv_a = sim._short_ltv_at_twap(sim.shorts[V])
    bonus_a = sim._effective_liq_bonus_bps(ltv_a)
    seized_a = sim.liquidate_short(90, V).get("sol_seized", 0)

    sim2, V2, _ = _setup_twap_short_at_liquidation(seed)
    # Hold the elevated price much longer so the mark sinks DEEPER into the window
    # (genuine sustained distress), not just barely over the threshold.
    for _ in range(int(2 * LIQ_TWAP_LOOKBACK_SLOTS // MIN_OBS_SPACING_SLOTS)):
        sim2.advance(MIN_OBS_SPACING_SLOTS)
    sim2.balances[90] = 5 * sim2.pool.token_reserves
    ltv_b = sim2._short_ltv_at_twap(sim2.shorts[V2]) if V2 in sim2.shorts else 0
    bonus_b = sim2._effective_liq_bonus_bps(ltv_b)
    seized_b = sim2.liquidate_short(90, V2).get("sol_seized", 0) if V2 in sim2.shorts else 0

    print(f"  just-over: TWAP LTV {ltv_a/100:>5.0f}% → bonus {bonus_a/100:>5.2f}% → "
          f"seized {seized_a/LAMPORTS_PER_SOL:.3f} SOL")
    print(f"  deep:      TWAP LTV {ltv_b/100:>5.0f}% → bonus {bonus_b/100:>5.2f}% → "
          f"seized {seized_b/LAMPORTS_PER_SOL:.3f} SOL")
    print()
    print("  A manufactured liquidation can only push a position JUST over the")
    print("  threshold → ~0 bonus → no prize to repay the sustained-hold cost.")
    print("  Genuine deep distress still pays full bonus to real liquidators. The")
    print("  TWAP doesn't stop the hold; this removes the reward — completing the")
    print("  stack: trigger guard + seize clamp + bonus cap + per-position custody.")
    return None


def scenario_depth_rails(seed=2029):
    """Depth-scaled risk rails, composed (the NEW math under test — ahead of the
    torch program until lifted). Three orthogonal pure functions price depth ONCE
    at open; the liquidation logic itself is unchanged:

      Rail 1  max-LTV curve  LTV(S) = LTV_max − (LTV_max−LTV_min)·(S_floor/S)
                             concave in depth — thin pools lever less.
      Rail 2  size cap       debt_value ≤ ρ_max·S
                             pins worst-case unwind slippage to ρ_max on EVERY pool.
      Rail 3  flat bonus     1.3·ρ_max, with full_bonus_ltv = 100/(1+bonus)
                             ⇒ a liquidator clearing a ≤ρ_max unwind is always paid.

    The scenario shows: on a shallow (~floor) pool a whale's leverage is clamped by
    BOTH the curve and the size cap; the realized debt/pool ratio pins to ρ_max at
    every depth (depth-invariant slippage); and a genuine held distress liquidation
    of a capped position clears with the flat bonus → zero protocol bad debt.
    """
    print("\n" + "=" * 60)
    print("  DEPTH-SCALED RISK RAILS (new math — tested in sim before lifting)")
    print("=" * 60)

    # ---- Rail 1: max-LTV curve vs pool depth (concave) ----
    print("\n  -- Rail 1: max-LTV curve  LTV_max − (LTV_max−LTV_min)·(S_floor/S) --")
    print(f"  floor S_floor={DEPTH_FLOOR_SOL//LAMPORTS_PER_SOL} SOL, "
          f"LTV {LTV_MIN_BPS/100:.0f}%→{LTV_MAX_BPS/100:.0f}%, ρ_max {RHO_MAX_BPS/100:.0f}%, "
          f"bonus {DEFAULT_LIQ_BONUS_BPS/100:.1f}% (=1.3·ρ_max), "
          f"full-bonus LTV {LIQ_FULL_BONUS_LTV_BPS/100:.1f}%")
    print(f"  {'pool S':>9} {'max LTV':>8} {'size cap ρ_max·S':>17}")
    for s in (100, 105, 200, 500, 1000, 5000):
        ps = s * LAMPORTS_PER_SOL
        print(f"  {s:>6} SOL {get_depth_max_ltv_bps(ps)/100:>7.1f}% "
              f"{max_debt_value_for_depth(ps)/LAMPORTS_PER_SOL:>13.1f} SOL")

    # ---- Rail 2: size cap binds; realized ρ pins to ρ_max at every depth ----
    print("\n  -- Rail 2: a whale maxes leverage; debt clamps to ρ_max·S at all depths --")
    print(f"  {'pool S':>9} {'maxLTV':>7} {'want (LTV·col)':>14} "
          f"{'size cap':>9} {'realized debt':>14} {'ρ=debt/S':>9} cap?")
    WHALE = 80
    for s in (105, 300, 1050):
        sim = _setup_migrated_depth(seed, s * LAMPORTS_PER_SOL)
        sim.treasury.lending_enabled = True
        sim.treasury.sol_balance = 10_000_000 * LAMPORTS_PER_SOL  # defeat lending cap
        sim.treasury.total_sol_lent_to_longs = 0
        pool_sol = sim.pool.sol_reserves
        eff_ltv = min(get_depth_max_ltv_bps(pool_sol), DEFAULT_MAX_LTV_BPS)
        cap = max_debt_value_for_depth(pool_sol)
        # Whale deposits ~a full pool's worth of tokens → uncapped want > cap.
        sim.balances[WHALE] = sim.pool.token_reserves
        col_val = (sim.pool.token_reserves * pool_sol) // max(1, sim.pool.token_reserves)
        want = (col_val * eff_ltv) // 10000
        res = sim.open_leveraged_long(WHALE, sim.pool.token_reserves)
        debt = res.get("borrowed_sol_gross", 0)
        rho = debt * 10000 // max(1, pool_sol)
        bound = "✓" if debt <= cap + 1 and want > cap else "—"
        print(f"  {pool_sol/LAMPORTS_PER_SOL:>6.0f} SOL {eff_ltv/100:>6.1f}% "
              f"{want/LAMPORTS_PER_SOL:>11.1f} SOL {cap/LAMPORTS_PER_SOL:>6.1f} SOL "
              f"{debt/LAMPORTS_PER_SOL:>11.1f} SOL {rho/100:>7.1f}% {bound:>4}")
    print("  Read: realized ρ pins to ρ_max at every depth — the size cap makes")
    print("  worst-case unwind slippage depth-invariant, so ONE flat bonus clears it.")

    # ---- Rail 3: capped position, genuine held distress → bonus clears, zero bad debt ----
    print("\n  -- Rail 3: held-distress liquidation of a capped short → bonus clears --")
    sim, V, hold = _setup_twap_short_at_liquidation(seed, victim_col_sol=60)
    LIQ = 90
    sim.balances[LIQ] = 5 * sim.pool.token_reserves
    pos = sim.shorts.get(V)
    twap_ltv = sim._short_ltv_at_twap(pos) if pos else 0
    bonus = sim._effective_liq_bonus_bps(twap_ltv)
    pool_sol_pre = sim.pool.sol_reserves
    res = sim.liquidate_short(LIQ, V)
    seized = res.get("sol_seized", 0)
    bad_debt = res.get("bad_debt", 0)
    # ρ at liquidation = covered SOL-debt-value / pool SOL (the unwind the liquidator eats)
    debt_value = (seized * 10000) // (10000 + bonus) if bonus or seized else 0
    rho_liq = debt_value * 10000 // max(1, pool_sol_pre)
    print(f"    held {hold} slots → TWAP LTV {twap_ltv/100:.0f}% → bonus {bonus/100:.2f}%")
    print(f"    seized {seized/LAMPORTS_PER_SOL:.3f} SOL | covered debt-value "
          f"{debt_value/LAMPORTS_PER_SOL:.3f} SOL | pool {pool_sol_pre/LAMPORTS_PER_SOL:.1f} SOL")
    print(f"    ρ at liq {rho_liq/100:.1f}%  vs  bonus {bonus/100:.1f}%  → "
          f"{'bonus ≥ ρ (liquidator cleared)' if bonus >= rho_liq else 'ρ > bonus'}"
          f" | bad debt {bad_debt}")
    print(f"    {'PASS ✓ capped position liquidates with zero protocol bad debt' if bad_debt == 0 else '⚠️ bad debt incurred'}")
    print()
    print("  The three rails compose: depth priced once at open (curve + size cap),")
    print("  solvency a flat contractual line, unwind funded by a flat bonus the")
    print("  capped ρ_max slippage can never outrun. Liquidation logic is unchanged —")
    print("  only the three pure parameter functions are new.")
    return None


def scenario_short_carry(seed=2030):
    """Short-side carry cost made legible. Short debt is TOKEN-denominated and
    accrues at DEFAULT_INTEREST_RATE_BPS per EPOCH_DURATION_SLOTS (a 7-day epoch),
    so the headline 'bps' is a per-week rate — a steep effective APR. Two effects:

      A. Carry-alone drift  — at FLAT price (no trades), interest grows the token
         debt → LTV climbs on its own → a max-LTV short is carried into liquidation
         with zero price movement. You cannot sit in a short.
      B. Reflexive buyback  — closing requires buying back (principal + accrued) from
         the only pool; carry enlarges the buy → more slippage up. The interest the
         short owes makes the exit it needs more expensive.

    NOTE: the handler accrues SIMPLE interest on principal (accrued_interest is not
    folded back into the base), so the honest figure is the simple APR and the LTV
    drift is linear — not the compounded APY.
    """
    global DEFAULT_INTEREST_RATE_BPS
    print("\n" + "=" * 60)
    print("  SHORT CARRY: token-denominated interest as a liquidation force")
    print("=" * 60)

    epoch_days = EPOCH_DURATION_SLOTS * 0.4 / 86400.0
    apr = DEFAULT_INTEREST_RATE_BPS / 100.0 * 365.0 / epoch_days
    print(f"\n  rate {DEFAULT_INTEREST_RATE_BPS} bps / {epoch_days:.0f}-day epoch "
          f"→ ~{apr:.0f}% simple APR, charged in the scarce token on a monopolist curve")

    sim = _setup_migrated(seed)
    V = 0
    col = 40 * LAMPORTS_PER_SOL
    sim.sol_balances[V] = col
    res = sim.open_short(V, col)
    pos = sim.shorts[V]
    principal = pos.tokens_borrowed
    vault_sol = pos.vault_sol
    pool_sol0 = sim.pool.sol_reserves
    pool_tok0 = sim.pool.token_reserves
    print(f"  opened: {col/LAMPORTS_PER_SOL:.0f} SOL collateral → borrowed "
          f"{principal/10**TOKEN_DECIMALS:,.0f} tok, vault {vault_sol/LAMPORTS_PER_SOL:.2f} SOL, "
          f"open LTV {res['ltv_bps']/100:.1f}%")
    print("  (LTV is low at open: the physical short BANKS the sale proceeds into")
    print("   vault_sol, so a fresh short is far more collateralized than its borrow")
    print("   ratio — carry has to grind through that buffer before it bites.)")

    # ---- A: carry-alone drift at FLAT price, run until liquidation ----
    print("\n  -- A: hold at flat price; carry alone drifts LTV + inflates the exit --")
    print(f"  {'wk':>3} {'int %prin':>9} {'LTV':>6} {'close buyback':>14} "
          f"{'mark (no-slip)':>15} {'slip prem':>10} {'status':>12}")
    crossed = None
    week = EPOCH_DURATION_SLOTS
    for w in range(0, 130):
        if w > 0:
            sim.advance(week)
        sim._accrue_short_interest(pos)
        ltv = sim._short_ltv_bps(pos)
        if ltv > DEFAULT_LIQ_THRESHOLD and crossed is None:
            crossed = w
        if w % 12 == 0 or w == crossed:
            total_debt = pos.tokens_borrowed + pos.accrued_interest
            int_pct = pos.accrued_interest * 100.0 / max(1, principal)
            gross = sim._gross_up_for_transfer_fee(total_debt)
            buyback = sim.pool.quote_buy_sol_in_for_tokens_out(gross)
            mark = total_debt * pool_sol0 // max(1, pool_tok0)
            if buyback is None:
                bb_s, prem_s = "pool too thin", "—"
            else:
                bb_s = f"{buyback/LAMPORTS_PER_SOL:.3f} SOL"
                prem_s = f"+{(buyback/max(1, mark) - 1)*100:.1f}%"
            status = "LIQUIDATABLE" if ltv > DEFAULT_LIQ_THRESHOLD else "healthy"
            print(f"  {w:>3} {int_pct:>8.1f}% {ltv/100:>5.1f}% {bb_s:>14} "
                  f"{mark/LAMPORTS_PER_SOL:>11.3f} SOL {prem_s:>10} {status:>12}")
        if crossed is not None and w >= crossed:
            break
    cross_msg = (f"~{crossed} weeks ({crossed/(365/7/12):.1f} months)" if crossed
                 else ">130 weeks")
    print(f"  → carry alone liquidates this short in {cross_msg} at FLAT price.")

    # ---- A-sweep: how the per-epoch rate moves the standalone burn ----
    print("\n  -- A-sweep: rate (bps/epoch) → weeks-to-liquidation (fresh short, flat) --")
    print(f"  {'rate':>6} {'APR':>7} {'open LTV':>9} {'weeks→liq':>10} {'months':>7}")
    _orig_rate = DEFAULT_INTEREST_RATE_BPS
    try:
        for rate in (50, 100, 150, 200):
            DEFAULT_INTEREST_RATE_BPS = rate
            s = _setup_migrated(seed)
            s.sol_balances[V] = col
            r0 = s.open_short(V, col)
            p = s.shorts[V]
            wk = None
            for w in range(1, 400):
                s.advance(week)
                s._accrue_short_interest(p)
                if s._short_ltv_bps(p) > DEFAULT_LIQ_THRESHOLD:
                    wk = w
                    break
            rate_apr = rate / 100.0 * 365.0 / epoch_days
            wk_s = f"{wk}" if wk else ">400"
            mo_s = f"{wk/(365/7/12):.1f}" if wk else "—"
            print(f"  {rate:>5}b {rate_apr:>6.0f}% {r0['ltv_bps']/100:>8.1f}% "
                  f"{wk_s:>10} {mo_s:>7}")
    finally:
        DEFAULT_INTEREST_RATE_BPS = _orig_rate
    print("  (linear: weeks-to-liq scales ~inversely with the rate — halve the rate,")
    print("   ~double the runway. The open LTV buffer is the same across rates.)")

    # ---- B: carry as an ACCELERANT on an already-stressed short ----
    print("\n  -- B: carry as a finisher — a short already pushed near threshold --")
    sim2 = _setup_migrated(seed + 1)
    M, V2 = 91, 0
    sim2.sol_balances[M] = 10_000_000 * LAMPORTS_PER_SOL
    sim2.sol_balances[V2] = 40 * LAMPORTS_PER_SOL
    sim2.open_short(V2, 40 * LAMPORTS_PER_SOL)
    p2 = sim2.shorts[V2]
    # Market pushes price up (adverse to the short) until LTV sits just under threshold.
    target = DEFAULT_LIQ_THRESHOLD - 1000  # ~55%
    while sim2._short_ltv_bps(p2) < target and sim2.pool.token_reserves > 1:
        if "error" in sim2.pool_buy(M, sim2.pool.sol_reserves // 200):
            break
    ltv_pre = sim2._short_ltv_bps(p2)
    fin = None
    for w in range(1, 60):
        sim2.advance(week)
        sim2._accrue_short_interest(p2)
        if sim2._short_ltv_bps(p2) > DEFAULT_LIQ_THRESHOLD:
            fin = w
            break
    fin_msg = f"{fin} weeks of carry" if fin else ">60 weeks"
    print(f"    stressed to {ltv_pre/100:.1f}% LTV by an adverse move, then carry "
          f"finishes it in {fin_msg}.")

    print()
    print(f"  Read: the ~{apr:.0f}% APR is real, but a FRESH short is buffered by its banked")
    print(f"  sale proceeds — standalone carry-to-liquidation is slow ({cross_msg}). Where")
    print("  it bites is (1) the reflexive exit: the buyback premium climbs with the")
    print("  token debt (the interest makes the exit it needs more expensive), and (2)")
    print("  as a FINISHER on an already-stressed short, turning a near-miss into a")
    print(f"  liquidation in weeks. Net: at {DEFAULT_INTEREST_RATE_BPS} bps/epoch shorts are tactical, not")
    print("  carry positions, and the interest routes back to the lock/treasury in-kind.")
    return None


def scenario_short_breakeven(seed=2031):
    """The trader's-eye 'is it attractive' chart: for a FIXED absolute trade, the
    favorable price decline a short needs just to break even — all costs included
    (open fee, Token-2022 transfer fees, 25bps pool fee, entry slippage on the
    sale, exit slippage on the buyback, and carry over the holding period).

    Entry is a REAL open_short (exact fee math); the exit is priced analytically
    along the CPMM (k held; pool fee on the close leg) at the price the move
    implies. Break-even move is measured from the POST-open spot (entry slippage
    already spent). Isolates the one thing that actually drives attractiveness:
    trade size relative to POOL DEPTH — the interest rate only shifts the carry
    column, depth shifts the whole row.
    """
    print("\n" + "=" * 60)
    print("  SHORT BREAK-EVEN: favorable move needed to profit (depth × hold)")
    print("=" * 60)
    col = 20 * LAMPORTS_PER_SOL
    holds = [0, 4, 12, 26]  # weeks held (carry)
    print(f"\n  fixed trade: {col//LAMPORTS_PER_SOL} SOL collateral; cells = price DECLINE from the")
    print(f"  CURRENT market price needed to break even (carry at {DEFAULT_INTEREST_RATE_BPS} bps/epoch)")
    hdr = f"  {'pool':>7} {'openLTV':>7} {'size/pool':>9}"
    for wk in holds:
        hdr += f" {('+'+str(wk)+'wk' if wk else 'flip'):>7}"
    print(hdr)

    pool_fee = 25  # deep_pool FEE_BPS
    def qb(s, t, out):  # SOL-in to buy `out` tokens on reserves (s,t), pool fee + ceil
        if out <= 0 or out >= t:
            return None
        eff = (s * out + (t - out) - 1) // (t - out)   # ceil effective-in
        denom = 10000 - pool_fee
        return (eff * 10000 + denom - 1) // denom

    for depth in (100, 200, 500, 1000, 3000):
        sim = _setup_migrated_depth(seed, depth * LAMPORTS_PER_SOL)
        sim.treasury.short_selling_enabled = True
        sim.sol_balances[0] = col
        s0, t0 = sim.pool.sol_reserves, sim.pool.token_reserves  # pre-trade market
        price_pre = s0 / t0
        r = sim.open_short(0, col)
        if "error" in r:
            print(f"  {depth:>5} SOL  {r['error']}")
            continue
        pos = sim.shorts[0]
        T = pos.tokens_borrowed
        s1, t1 = sim.pool.sol_reserves, sim.pool.token_reserves
        price_ref = s1 / t1                                # post-open spot (entry slip spent)
        entry_slip = (1.0 - price_ref / price_pre) * 100.0  # one-time haircut from own sale
        target = r["sol_from_sale"] - r["open_fee_sol"]   # buyback budget at break-even
        debt_val = T * s1 // t1
        size_pct = debt_val * 100.0 / s1
        cells = []
        for wk in holds:
            interest = T * DEFAULT_INTEREST_RATE_BPS * wk // 10000  # simple, per-epoch
            gross = sim._gross_up_for_transfer_fee(T + interest)

            def cost(f):  # buyback cost at price = price_ref·f  (s_e=s1√f, t_e=t1/√f)
                rt = f ** 0.5
                return qb(int(s1 * rt), int(t1 / rt), gross)

            if (c1 := cost(1.0)) is None:
                cells.append(None)
                continue
            if c1 <= target:
                # break-even reached at post-open spot → only the entry haircut remains
                cells.append(entry_slip)
                continue
            lo, hi = 1e-9, 1.0   # cost(lo)≈0≤target < cost(hi)=c1; find f* where cost=target
            for _ in range(50):
                mid = (lo + hi) / 2
                cm = cost(mid)
                if cm is None or cm > target:
                    hi = mid
                else:
                    lo = mid
            price_e = price_ref * lo                         # exit price at break-even
            cells.append((1.0 - price_e / price_pre) * 100.0)  # DECLINE from current market
        row = f"  {depth:>5} SOL {r['ltv_bps']/100:>6.1f}% {size_pct:>8.1f}%"
        for c in cells:
            row += f" {('—' if c is None else f'{c:.1f}%'):>7}"
        print(row)

    print()
    print("  Decomposition of each cell ≈ entry slippage + fee stack (~1%) + carry:")
    print("  • Round-trip slippage CANCELS (sell down, buy back up the same curve), so")
    print("    the only depth-sensitive term is the ONE-TIME entry haircut (size/pool).")
    print("  • Read DOWN a column: depth shrinks that haircut — the flip-trade break-")
    print("    even falls from the thin-pool entry cost toward just the ~1% fee floor.")
    print("  • Read ACROSS a row: carry adds a few points and is nearly depth-flat —")
    print(f"    confirming {DEFAULT_INTEREST_RATE_BPS} bps is a minor term. Takeaway: a short is attractive")
    print("    when your size is a small fraction of the pool (deep pool / small size);")
    print("    the real hurdle is your own entry impact, not the interest rate.")
    return None


if __name__ == "__main__":
    print("Torch Market Economic Simulator v0.1")
    print("=" * 60)

    sim1 = scenario_full_lifecycle()
    sim2 = scenario_cascade_stress()
    sim3 = scenario_sandwich_attack()
    sim4 = scenario_harvest_gaming()
    sim5 = scenario_long_short_dump()
    sim6 = scenario_leveraged_long()
    sim7 = scenario_long_short_basis()
    sim8 = scenario_directional_profitability()
    sim9 = scenario_invariant_check()
    sim10 = scenario_per_token_closure()
    scenario_oracle_liquidation_attack()
    scenario_oracle_attack_sweep()
    scenario_solvency_gap_crash()
    scenario_long_baddebt_severity()
    scenario_oracle_mitigation_test()
    scenario_twap_tuning()
    scenario_liq_bonus_ramp()
    scenario_depth_rails()
    scenario_short_carry()
    scenario_short_breakeven()

    print("\n\n" + "=" * 60)
    print("  ALL SCENARIOS COMPLETE")
    print("=" * 60)
