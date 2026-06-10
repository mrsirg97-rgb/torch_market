"""Domain models: bonding curve, DeepPool CPMM (+ keeperless TWAP oracle),
treasury, positions, event log."""
# Extracted from the former monolithic torch_sim.py (split 2026-06-09).
# torch_sim.py remains the entry point / public surface.

from __future__ import annotations
import math
from dataclasses import dataclass, field
from typing import Optional
from enum import Enum

from constants import *

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
    def lending_assets(self) -> int:
        """[F-2] Principal pool — the STICKY unlock-gate basis: physical float
        + outstanding receivables. Invariant under borrow/repay (a borrow moves
        SOL from sol_balance to total_sol_lent_to_longs); grows with fees +
        interest; shrinks only on a bad-debt write-off (which re-locks the
        gate — documented intent, lending-unlock.md)."""
        return self.sol_balance + self.total_sol_lent_to_longs

    @property
    def available_to_lend(self) -> int:
        """[F-2] FCFS capacity: the physical float. Already net of lent SOL —
        no utilization cap, no lent subtraction (full-drain by design)."""
        return max(0, self.sol_balance)

    @property
    def utilization_bps(self) -> int:
        assets = self.lending_assets
        if assets <= 0:
            return 0
        return (self.total_sol_lent_to_longs * 10000) // assets


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
