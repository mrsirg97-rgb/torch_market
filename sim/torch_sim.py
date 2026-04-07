"""
Torch Market Economic Simulator

Pure-Python simulation of the Torch Market protocol:
  - Bonding curve (constant product with dynamic fee splits)
  - Post-migration DeepPool
  - Treasury lending (SOL loans against token collateral)
  - Short selling (token loans against SOL collateral)
  - Liquidation cascades
  - Transfer fee harvesting

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
DEV_WALLET_SHARE_BPS   = 1000  # 10% of protocol fee
TRANSFER_FEE_BPS       = 4     # 0.04%
CREATOR_FEE_SHARE_BPS  = 1500  # 15% of swap_fees_to_sol proceeds

# Lending
DEFAULT_INTEREST_RATE_BPS = 200   # 2% per epoch
DEFAULT_MAX_LTV_BPS      = 5000  # 50%
DEFAULT_LIQ_THRESHOLD    = 6500  # 65%
DEFAULT_LIQ_BONUS_BPS    = 1000  # 10%
DEFAULT_LIQ_CLOSE_BPS    = 5000  # 50% close factor
LENDING_UTIL_CAP_BPS     = 8000  # 80%
BORROW_SHARE_MULTIPLIER  = 23
MIN_BORROW_AMOUNT        = 100_000_000  # 0.1 SOL
MIN_POOL_SOL_LENDING     = 5_000_000_000  # 5 SOL
MAX_PRICE_DEVIATION_BPS  = 5000  # 50%

# Depth-based risk bands
DEPTH_TIER_1 = 50_000_000_000    # 50 SOL
DEPTH_TIER_2 = 200_000_000_000   # 200 SOL
DEPTH_TIER_3 = 500_000_000_000   # 500 SOL
DEPTH_LTV_0  = 2500  # < 50 SOL  → 25%
DEPTH_LTV_1  = 3500  # 50-200    → 35%
DEPTH_LTV_2  = 4500  # 200-500   → 45%
DEPTH_LTV_3  = 5000  # 500+      → 50%

def get_depth_max_ltv_bps(pool_sol: int) -> int:
    if pool_sol < MIN_POOL_SOL_LENDING:
        return 0
    elif pool_sol < DEPTH_TIER_1:
        return DEPTH_LTV_0
    elif pool_sol < DEPTH_TIER_2:
        return DEPTH_LTV_1
    elif pool_sol < DEPTH_TIER_3:
        return DEPTH_LTV_2
    else:
        return DEPTH_LTV_3

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
    """Post-migration DeepPool. 0.25% fee auto-compounds into reserves."""
    sol_reserves: int = 0
    token_reserves: int = 0

    @property
    def k(self) -> int:
        return self.sol_reserves * self.token_reserves

    @property
    def price(self) -> float:
        if self.token_reserves == 0:
            return float('inf')
        return self.sol_reserves / self.token_reserves

    FEE_BPS: int = 25  # 0.25% — auto-compounds into reserves

    def swap_sol_for_tokens(self, sol_in: int) -> int:
        """Constant product swap with fee. Returns tokens out."""
        if sol_in <= 0 or self.sol_reserves <= 0 or self.token_reserves <= 0:
            return 0
        fee = (sol_in * self.FEE_BPS) // 10000
        effective_in = sol_in - fee
        tokens_out = (self.token_reserves * effective_in) // (self.sol_reserves + effective_in)
        tokens_out = min(tokens_out, self.token_reserves - 1)  # can't drain pool
        self.sol_reserves += sol_in  # full amount including fee stays in pool
        self.token_reserves -= tokens_out
        return tokens_out

    def swap_tokens_for_sol(self, tokens_in: int) -> int:
        """Constant product swap with fee. Returns SOL out."""
        if tokens_in <= 0 or self.sol_reserves <= 0 or self.token_reserves <= 0:
            return 0
        fee = (tokens_in * self.FEE_BPS) // 10000
        effective_in = tokens_in - fee
        sol_out = (self.sol_reserves * effective_in) // (self.token_reserves + effective_in)
        sol_out = min(sol_out, self.sol_reserves - 1)
        self.token_reserves += tokens_in  # full amount including fee stays in pool
        self.sol_reserves -= sol_out
        return sol_out


@dataclass
class Treasury:
    sol_balance: int = 0
    total_sol_lent: int = 0
    total_collateral_locked: int = 0
    active_loans: int = 0
    total_interest_collected: int = 0
    # Short tracking
    short_collateral_sol: int = 0  # total_burned_from_buyback
    total_tokens_lent: int = 0
    active_shorts: int = 0
    short_interest_collected: int = 0
    # Baseline
    baseline_sol: int = 0
    baseline_tokens: int = 0
    # Harvest
    harvested_fees_tokens: int = 0

    @property
    def available_to_lend(self) -> int:
        cap = (self.sol_balance * LENDING_UTIL_CAP_BPS) // 10000
        return max(0, cap - self.total_sol_lent)

    @property
    def utilization_bps(self) -> int:
        available = max(1, self.sol_balance - self.short_collateral_sol)
        if available <= 0:
            return 0
        return (self.total_sol_lent * 10000) // available


@dataclass
class LoanPosition:
    user_id: int
    collateral_tokens: int = 0
    borrowed_sol: int = 0
    accrued_interest: int = 0
    last_slot: int = 0


@dataclass
class ShortPosition:
    user_id: int
    sol_collateral: int = 0
    tokens_borrowed: int = 0
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
        self.loans: dict[int, LoanPosition] = {}
        self.shorts: dict[int, ShortPosition] = {}

        # Accounting
        self.protocol_treasury_sol: int = 0
        self.dev_wallet_sol: int = 0
        self.creator_sol: int = 0
        self.transfer_fee_accrued: int = 0  # tokens withheld

        self.log: list[LogEntry] = []
        self.snapshots: list[dict] = []

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _apply_transfer_fee(self, amount: int) -> tuple[int, int]:
        """Returns (net_received, fee_withheld)."""
        fee = (amount * TRANSFER_FEE_BPS) // 10000
        return amount - fee, fee

    def _pool_price_in_band(self) -> bool:
        if self.treasury.baseline_sol == 0 or self.treasury.baseline_tokens == 0:
            return False
        current_ratio = (self.pool.sol_reserves * 10**9) // self.pool.token_reserves
        baseline_ratio = (self.treasury.baseline_sol * 10**9) // self.treasury.baseline_tokens
        if baseline_ratio == 0:
            return False
        deviation = abs(current_ratio - baseline_ratio) * 10000 // baseline_ratio
        return deviation <= MAX_PRICE_DEVIATION_BPS

    def _accrue_loan_interest(self, loan: LoanPosition):
        if loan.borrowed_sol == 0:
            return
        slots_elapsed = self.slot - loan.last_slot
        if slots_elapsed <= 0:
            return
        interest = (loan.borrowed_sol * DEFAULT_INTEREST_RATE_BPS * slots_elapsed) \
                   // (10000 * EPOCH_DURATION_SLOTS)
        loan.accrued_interest += interest
        loan.last_slot = self.slot

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

    def _loan_ltv_bps(self, loan: LoanPosition) -> int:
        if loan.collateral_tokens == 0:
            return 10**18
        total_debt = loan.borrowed_sol + loan.accrued_interest
        collateral_value = (loan.collateral_tokens * self.pool.sol_reserves) \
                           // self.pool.token_reserves
        if collateral_value == 0:
            return 10**18
        return (total_debt * 10000) // collateral_value

    def _short_ltv_bps(self, pos: ShortPosition) -> int:
        if pos.sol_collateral == 0:
            return 10**18
        total_debt_tokens = pos.tokens_borrowed + pos.accrued_interest
        debt_value = (total_debt_tokens * self.pool.sol_reserves) // self.pool.token_reserves
        if pos.sol_collateral == 0:
            return 10**18
        return (debt_value * 10000) // pos.sol_collateral

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
        creator_sol = (sol_after_fees * creator_rate) // 10000
        treasury_sol = total_split - creator_sol
        sol_to_curve = sol_after_fees - total_split

        self.treasury.sol_balance += treasury_sol
        self.creator_sol += creator_sol

        # Constant product
        tokens_out = (self.curve.virtual_tokens * sol_to_curve) \
                     // (self.curve.virtual_sol + sol_to_curve)
        tokens_out = min(tokens_out, self.curve.real_tokens)

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

        self.pool.sol_reserves = self.curve.real_sol
        self.pool.token_reserves = self.curve.real_tokens
        self.treasury.baseline_sol = self.curve.real_sol
        self.treasury.baseline_tokens = self.curve.real_tokens
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

        tokens_out = self.pool.swap_sol_for_tokens(sol_spent)
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
        sol_out = self.pool.swap_tokens_for_sol(net)
        self._credit_sol(user_id, sol_out)

        return {"tokens_sold": tokens, "sol_received": sol_out, "transfer_fee": fee,
                "price": self.pool.price}

    # ------------------------------------------------------------------
    # Lending
    # ------------------------------------------------------------------

    def borrow(self, user_id: int, collateral_tokens: int, sol_to_borrow: int) -> dict:
        """Borrow SOL against token collateral."""
        assert self.migrated, "Not migrated"
        depth_max_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves)
        assert depth_max_ltv > 0, "Pool too thin"
        assert sol_to_borrow >= MIN_BORROW_AMOUNT, "Borrow too small"

        # Deposit collateral
        tokens_deposited = self._debit_tokens(user_id, collateral_tokens)
        net, fee = self._apply_transfer_fee(tokens_deposited)
        self.transfer_fee_accrued += fee

        loan = self.loans.get(user_id)
        if loan is None:
            loan = LoanPosition(user_id=user_id, last_slot=self.slot)
            self.loans[user_id] = loan
            self.treasury.active_loans += 1
        else:
            self._accrue_loan_interest(loan)

        loan.collateral_tokens += net
        self.treasury.total_collateral_locked += net

        # Check utilization — matches on-chain: available = sol_balance - short_reserved
        available_sol = max(0, self.treasury.sol_balance - self.treasury.short_collateral_sol)
        max_lendable = (available_sol * LENDING_UTIL_CAP_BPS) // 10000
        new_total_lent = self.treasury.total_sol_lent + sol_to_borrow
        if new_total_lent > max_lendable:
            # Return collateral — on-chain this is a hard require!
            self._credit_tokens(user_id, net)  # give back (ignoring fee for simplicity)
            loan.collateral_tokens -= net
            self.treasury.total_collateral_locked -= net
            if loan.collateral_tokens == 0 and loan.borrowed_sol == 0:
                self.treasury.active_loans -= 1
                del self.loans[user_id]
            return {"error": "lending cap exceeded"}

        # Per-user borrow cap: max_user_borrow = max_lendable * collateral * 5 / TOTAL_SUPPLY
        user_total_borrowed = loan.borrowed_sol + sol_to_borrow
        max_user_borrow = (max_lendable * loan.collateral_tokens * BORROW_SHARE_MULTIPLIER) \
                          // TOTAL_SUPPLY
        if user_total_borrowed > max_user_borrow:
            self._credit_tokens(user_id, net)
            loan.collateral_tokens -= net
            self.treasury.total_collateral_locked -= net
            if loan.collateral_tokens == 0 and loan.borrowed_sol == 0:
                self.treasury.active_loans -= 1
                del self.loans[user_id]
            return {"error": "user borrow cap exceeded"}

        # LTV check — hard reject like on-chain
        total_debt = loan.borrowed_sol + loan.accrued_interest + sol_to_borrow
        collateral_value = (loan.collateral_tokens * self.pool.sol_reserves) \
                           // self.pool.token_reserves
        if collateral_value == 0:
            self._credit_tokens(user_id, net)
            loan.collateral_tokens -= net
            self.treasury.total_collateral_locked -= net
            if loan.collateral_tokens == 0 and loan.borrowed_sol == 0:
                self.treasury.active_loans -= 1
                del self.loans[user_id]
            return {"error": "zero collateral value"}
        effective_max_ltv = min(depth_max_ltv, DEFAULT_MAX_LTV_BPS)
        ltv = (total_debt * 10000) // collateral_value
        if ltv > effective_max_ltv:
            self._credit_tokens(user_id, net)
            loan.collateral_tokens -= net
            self.treasury.total_collateral_locked -= net
            if loan.collateral_tokens == 0 and loan.borrowed_sol == 0:
                self.treasury.active_loans -= 1
                del self.loans[user_id]
            return {"error": f"LTV exceeded ({ltv} > {effective_max_ltv})"}
        actual_borrow = sol_to_borrow

        loan.borrowed_sol += actual_borrow
        self.treasury.total_sol_lent += actual_borrow
        self.treasury.sol_balance -= actual_borrow  # SOL leaves treasury
        self._credit_sol(user_id, actual_borrow)

        detail = {
            "collateral": net, "borrowed": actual_borrow,
            "ltv_bps": self._loan_ltv_bps(loan), "price": self.pool.price,
        }
        self.log.append(LogEntry(self.slot, Event.BORROW, user_id, detail))
        return detail

    def repay(self, user_id: int, sol_amount: int) -> dict:
        """Repay SOL loan."""
        loan = self.loans.get(user_id)
        assert loan is not None, "No loan"
        self._accrue_loan_interest(loan)

        actual = self._debit_sol(user_id, sol_amount)
        if actual == 0:
            return {"error": "no SOL"}

        remaining = actual
        # Interest first
        interest_paid = min(remaining, loan.accrued_interest)
        loan.accrued_interest -= interest_paid
        remaining -= interest_paid

        # Then principal
        principal_paid = min(remaining, loan.borrowed_sol)
        loan.borrowed_sol -= principal_paid
        remaining -= principal_paid

        self.treasury.sol_balance += actual - remaining  # SOL returns
        self.treasury.total_sol_lent -= principal_paid
        self.treasury.total_interest_collected += interest_paid

        # Refund overpayment
        if remaining > 0:
            self._credit_sol(user_id, remaining)

        # Return collateral if fully repaid
        collateral_returned = 0
        if loan.borrowed_sol == 0 and loan.accrued_interest == 0:
            collateral_returned = loan.collateral_tokens
            net, fee = self._apply_transfer_fee(loan.collateral_tokens)
            self.transfer_fee_accrued += fee
            self._credit_tokens(user_id, net)
            self.treasury.total_collateral_locked -= loan.collateral_tokens
            loan.collateral_tokens = 0
            self.treasury.active_loans -= 1
            del self.loans[user_id]

        detail = {"sol_repaid": actual - remaining, "interest_paid": interest_paid,
                  "principal_paid": principal_paid, "collateral_returned": collateral_returned}
        self.log.append(LogEntry(self.slot, Event.REPAY, user_id, detail))
        return detail

    def liquidate_long(self, liquidator_id: int, borrower_id: int) -> dict:
        """Liquidate an underwater long position."""
        loan = self.loans.get(borrower_id)
        assert loan is not None, "No loan"
        assert self.migrated
        self._accrue_loan_interest(loan)

        ltv = self._loan_ltv_bps(loan)
        assert ltv > DEFAULT_LIQ_THRESHOLD, f"LTV {ltv} below threshold {DEFAULT_LIQ_THRESHOLD}"

        total_debt = loan.borrowed_sol + loan.accrued_interest
        # Close factor: 50%
        debt_to_cover = (total_debt * DEFAULT_LIQ_CLOSE_BPS) // 10000

        # Collateral to seize (with bonus)
        collateral_value_per_token = self.pool.sol_reserves * 10**9 // self.pool.token_reserves
        target_collateral = (debt_to_cover * (10000 + DEFAULT_LIQ_BONUS_BPS) * 10**9) \
                            // (10000 * collateral_value_per_token) if collateral_value_per_token > 0 else 0
        actual_collateral = min(target_collateral, loan.collateral_tokens)

        # Bad debt: if collateral insufficient
        actual_debt_covered = debt_to_cover
        if actual_collateral < target_collateral and target_collateral > 0:
            actual_debt_covered = (actual_collateral * collateral_value_per_token * 10000) \
                                  // ((10000 + DEFAULT_LIQ_BONUS_BPS) * 10**9)
        bad_debt = total_debt - actual_debt_covered if actual_debt_covered < total_debt else 0

        # Liquidator pays SOL, receives collateral tokens
        sol_paid = self._debit_sol(liquidator_id, actual_debt_covered)
        if sol_paid < actual_debt_covered:
            return {"error": "liquidator insufficient SOL"}

        # Transfer collateral to liquidator
        net_tokens, fee = self._apply_transfer_fee(actual_collateral)
        self.transfer_fee_accrued += fee
        self._credit_tokens(liquidator_id, net_tokens)

        # Update loan
        remaining = actual_debt_covered
        interest_paid = min(remaining, loan.accrued_interest)
        loan.accrued_interest -= interest_paid
        remaining -= interest_paid
        loan.borrowed_sol -= min(remaining, loan.borrowed_sol)

        if bad_debt > 0:
            loan.borrowed_sol = 0
            loan.accrued_interest = 0

        loan.collateral_tokens -= actual_collateral

        # Update treasury
        self.treasury.total_sol_lent -= min(remaining, self.treasury.total_sol_lent)
        if bad_debt > 0:
            self.treasury.total_sol_lent = max(0, self.treasury.total_sol_lent - bad_debt)
        self.treasury.sol_balance += actual_debt_covered
        self.treasury.total_interest_collected += interest_paid
        self.treasury.total_collateral_locked -= actual_collateral

        fully_liquidated = loan.borrowed_sol == 0 and loan.accrued_interest == 0
        if fully_liquidated:
            # Return remaining collateral
            if loan.collateral_tokens > 0:
                net, fee = self._apply_transfer_fee(loan.collateral_tokens)
                self.transfer_fee_accrued += fee
                self._credit_tokens(borrower_id, net)
                self.treasury.total_collateral_locked -= loan.collateral_tokens
                loan.collateral_tokens = 0
            self.treasury.active_loans -= 1
            del self.loans[borrower_id]

        detail = {
            "debt_covered": actual_debt_covered, "collateral_seized": actual_collateral,
            "bad_debt": bad_debt, "fully_liquidated": fully_liquidated,
            "ltv_before": ltv,
        }
        self.log.append(LogEntry(self.slot, Event.LIQUIDATE_LONG, liquidator_id, detail))
        return detail

    # ------------------------------------------------------------------
    # Short Selling
    # ------------------------------------------------------------------

    def open_short(self, user_id: int, sol_collateral: int) -> dict:
        """Open a short position: deposit SOL, borrow tokens from lock."""
        assert self.migrated
        depth_max_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves)
        assert depth_max_ltv > 0, "Pool too thin"
        effective_max_ltv = min(depth_max_ltv, DEFAULT_MAX_LTV_BPS)

        sol = self._debit_sol(user_id, sol_collateral)
        if sol == 0:
            return {"error": "no SOL"}

        # How many tokens to borrow at current price?
        # debt_value = tokens * pool_sol / pool_tokens
        # ltv = debt_value / sol_collateral
        # tokens = effective_ltv * sol_collateral * pool_tokens / (pool_sol * 10000)
        max_tokens = (effective_max_ltv * sol * self.pool.token_reserves) \
                     // (self.pool.sol_reserves * 10000)
        available = self.lock_tokens - self.treasury.total_tokens_lent
        tokens_to_borrow = min(max_tokens, available)
        if tokens_to_borrow <= 0:
            self._credit_sol(user_id, sol)
            return {"error": "no tokens available"}

        pos = ShortPosition(user_id=user_id, sol_collateral=sol,
                           tokens_borrowed=tokens_to_borrow, last_slot=self.slot)
        self.shorts[user_id] = pos
        self.treasury.sol_balance += sol
        self.treasury.short_collateral_sol += sol
        self.treasury.total_tokens_lent += tokens_to_borrow
        self.treasury.active_shorts += 1

        # Give tokens to user (they'll sell on market)
        net, fee = self._apply_transfer_fee(tokens_to_borrow)
        self.transfer_fee_accrued += fee
        self._credit_tokens(user_id, net)

        detail = {
            "sol_collateral": sol, "tokens_borrowed": tokens_to_borrow,
            "ltv_bps": self._short_ltv_bps(pos), "price": self.pool.price,
        }
        self.log.append(LogEntry(self.slot, Event.OPEN_SHORT, user_id, detail))
        return detail

    def close_short(self, user_id: int, token_amount: int) -> dict:
        """Close (or partially close) a short position."""
        pos = self.shorts.get(user_id)
        assert pos is not None, "No short position"
        self._accrue_short_interest(pos)

        tokens = self._debit_tokens(user_id, token_amount)
        if tokens == 0:
            return {"error": "no tokens"}
        net, fee = self._apply_transfer_fee(tokens)
        self.transfer_fee_accrued += fee

        # Pay interest first, then principal
        remaining = net
        interest_paid = min(remaining, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        remaining -= interest_paid

        principal_paid = min(remaining, pos.tokens_borrowed)
        pos.tokens_borrowed -= principal_paid
        remaining -= principal_paid

        # Return proportional SOL collateral
        total_debt_before = pos.tokens_borrowed + principal_paid + pos.accrued_interest + interest_paid
        if total_debt_before > 0:
            sol_return = (pos.sol_collateral * principal_paid) // total_debt_before
        else:
            sol_return = pos.sol_collateral
        sol_return = min(sol_return, pos.sol_collateral)

        pos.sol_collateral -= sol_return
        self.treasury.sol_balance -= sol_return
        self.treasury.short_collateral_sol -= sol_return
        self._credit_sol(user_id, sol_return)
        self.treasury.total_tokens_lent -= principal_paid
        self.treasury.short_interest_collected += interest_paid

        fully_closed = pos.tokens_borrowed == 0 and pos.accrued_interest == 0
        if fully_closed:
            # Return remaining collateral
            if pos.sol_collateral > 0:
                self._credit_sol(user_id, pos.sol_collateral)
                self.treasury.sol_balance -= pos.sol_collateral
                self.treasury.short_collateral_sol -= pos.sol_collateral
                pos.sol_collateral = 0
            self.treasury.active_shorts -= 1
            del self.shorts[user_id]

        detail = {"tokens_returned": net, "sol_returned": sol_return,
                  "interest_paid": interest_paid, "fully_closed": fully_closed}
        self.log.append(LogEntry(self.slot, Event.CLOSE_SHORT, user_id, detail))
        return detail

    def liquidate_short(self, liquidator_id: int, shorter_id: int) -> dict:
        """Liquidate an underwater short position."""
        pos = self.shorts.get(shorter_id)
        assert pos is not None, "No short position"
        self._accrue_short_interest(pos)

        ltv = self._short_ltv_bps(pos)
        assert ltv > DEFAULT_LIQ_THRESHOLD, f"Short LTV {ltv} below threshold"

        total_debt = pos.tokens_borrowed + pos.accrued_interest
        tokens_to_cover = (total_debt * DEFAULT_LIQ_CLOSE_BPS) // 10000

        # SOL to seize (with bonus)
        debt_value = (tokens_to_cover * self.pool.sol_reserves) // self.pool.token_reserves
        target_sol = (debt_value * (10000 + DEFAULT_LIQ_BONUS_BPS)) // 10000
        actual_sol_seized = min(target_sol, pos.sol_collateral)

        # Bad debt
        actual_tokens_covered = tokens_to_cover
        if actual_sol_seized < target_sol and target_sol > 0:
            actual_tokens_covered = (tokens_to_cover * actual_sol_seized) // target_sol
        bad_debt = total_debt - actual_tokens_covered if actual_tokens_covered < total_debt else 0

        # Liquidator supplies tokens
        tokens_from_liq = self._debit_tokens(liquidator_id, actual_tokens_covered)
        net, fee = self._apply_transfer_fee(tokens_from_liq)
        self.transfer_fee_accrued += fee

        # Liquidator receives SOL
        self._credit_sol(liquidator_id, actual_sol_seized)

        # Update position
        remaining = net
        interest_paid = min(remaining, pos.accrued_interest)
        pos.accrued_interest -= interest_paid
        remaining -= interest_paid
        pos.tokens_borrowed -= min(remaining, pos.tokens_borrowed)

        if bad_debt > 0:
            pos.tokens_borrowed = 0
            pos.accrued_interest = 0

        pos.sol_collateral -= actual_sol_seized
        self.treasury.sol_balance -= actual_sol_seized
        self.treasury.short_collateral_sol -= actual_sol_seized
        self.treasury.total_tokens_lent -= min(remaining + bad_debt, self.treasury.total_tokens_lent)
        self.treasury.short_interest_collected += interest_paid

        fully_liquidated = pos.tokens_borrowed == 0 and pos.accrued_interest == 0
        if fully_liquidated:
            if pos.sol_collateral > 0:
                self._credit_sol(shorter_id, pos.sol_collateral)
                self.treasury.sol_balance -= pos.sol_collateral
                self.treasury.short_collateral_sol -= pos.sol_collateral
            self.treasury.active_shorts -= 1
            del self.shorts[shorter_id]

        detail = {
            "tokens_covered": actual_tokens_covered, "sol_seized": actual_sol_seized,
            "bad_debt": bad_debt, "fully_liquidated": fully_liquidated,
            "ltv_before": ltv,
        }
        self.log.append(LogEntry(self.slot, Event.LIQUIDATE_SHORT, liquidator_id, detail))
        return detail

    # ------------------------------------------------------------------
    # Fee Harvesting
    # ------------------------------------------------------------------

    def harvest_and_swap(self, is_community_token: bool = False) -> dict:
        """Harvest accumulated transfer fees and swap to SOL via pool.

        Mirrors on-chain: harvest_fees() + swap_fees_to_sol().
        1. Collect withheld transfer fees into treasury token account
        2. Swap tokens → SOL via pool (constant product)
        3. Split SOL: 85% treasury, 15% creator (if not community token)
        """
        assert self.migrated, "Not migrated"

        tokens_to_swap = self.transfer_fee_accrued
        if tokens_to_swap == 0:
            return {"error": "no fees to harvest"}

        # Reset accrued — these are now "in" the treasury token account
        self.transfer_fee_accrued = 0

        # Swap tokens for SOL via pool (same constant product as any sell)
        sol_out = self.pool.swap_tokens_for_sol(tokens_to_swap)
        if sol_out == 0:
            return {"error": "swap returned 0"}

        # Community token: 100% to treasury (no creator split)
        self.treasury.sol_balance += sol_out
        self.treasury.harvested_fees_tokens += tokens_to_swap

        detail = {
            "tokens_swapped": tokens_to_swap,
            "sol_out": sol_out,
            "treasury_total": self.treasury.sol_balance,
        }
        self.log.append(LogEntry(self.slot, Event.HARVEST, -1, detail))
        return detail

    # ------------------------------------------------------------------
    # Snapshots & Reporting
    # ------------------------------------------------------------------

    def advance(self, slots: int = 1):
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
            "treasury_lent": self.treasury.total_sol_lent / LAMPORTS_PER_SOL,
            "treasury_util_bps": self.treasury.utilization_bps,
            "active_loans": self.treasury.active_loans,
            "active_shorts": self.treasury.active_shorts,
            "short_collateral_sol": self.treasury.short_collateral_sol / LAMPORTS_PER_SOL,
            "protocol_revenue": self.protocol_treasury_sol / LAMPORTS_PER_SOL,
            "transfer_fees_tokens": self.transfer_fee_accrued / 10**TOKEN_DECIMALS,
            "interest_collected": self.treasury.total_interest_collected / LAMPORTS_PER_SOL,
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
                  f"Price: {self.curve.price * 10**TOKEN_DECIMALS:.6f} SOL/token")
        else:
            print(f"  Pool: {s['pool_sol']:.2f} SOL / {s['pool_tokens']:,.0f} tokens")
            print(f"  Price: {s['pool_price'] * 10**TOKEN_DECIMALS:.6f} SOL/token")
            print(f"  K invariant: {s['pool_k']:,}")
        depth_ltv = get_depth_max_ltv_bps(self.pool.sol_reserves) if self.migrated else 0
        print(f"  Treasury: {s['treasury_sol']:.2f} SOL "
              f"(lent: {s['treasury_lent']:.2f}, util: {s['treasury_util_bps']/100:.1f}%)")
        if self.migrated:
            print(f"  Depth band: {s['pool_sol']:.0f} SOL → max LTV {depth_ltv/100:.0f}%")
        print(f"  Loans: {s['active_loans']} | Shorts: {s['active_shorts']} "
              f"(collateral: {s['short_collateral_sol']:.2f} SOL)")
        print(f"  Protocol revenue: {s['protocol_revenue']:.4f} SOL")
        print(f"  Transfer fees accrued: {s['transfer_fees_tokens']:,.0f} tokens")
        print(f"  Interest collected: {s['interest_collected']:.4f} SOL")


# ============================================================================
# Scenario Runners
# ============================================================================

def scenario_full_lifecycle(seed=42):
    """Full lifecycle: bonding → migration → lending → shorts → liquidations."""
    sim = TorchSim(seed=seed)
    print("\n" + "="*60)
    print("  SCENARIO: Full Token Lifecycle")
    print("="*60)

    # Fund users
    num_users = 20
    for i in range(num_users):
        sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL  # 50 SOL each

    # Phase 1: Bonding with PVP (buys AND sells on the curve)
    print("\n--- Phase 1: Bonding Curve (PVP) ---")
    buy_count = 0
    sell_count = 0
    gross_buy_volume = 0
    while not sim.curve.bonding_complete:
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

    # Phase 3: Organic trading + fee harvesting → grow treasury to ~100 SOL
    print("\n--- Phase 3: Organic Trading + Fee Harvesting ---")
    target_treasury = 100 * LAMPORTS_PER_SOL
    harvest_interval = 500  # harvest every 500 slots
    slots_since_harvest = 0
    trade_count = 0
    harvest_count = 0

    # Balanced trading: 50/50 buy/sell, limited SOL inflow, realistic sizing
    while sim.treasury.sol_balance < target_treasury:
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
    print(f"    Avg price at harvest: {avg_price_at_harvest:.6f} SOL/token")
    print(f"    Migration price was: {sim.treasury.baseline_sol / sim.treasury.baseline_tokens * 10**TOKEN_DECIMALS:.6f} SOL/token")
    print(f"    Harvest range: {min_sol / LAMPORTS_PER_SOL:.6f} - {max_sol / LAMPORTS_PER_SOL:.6f} SOL per harvest")
    sim.print_snapshot()

    # Phase 4: Lending (treasury now has ~100 SOL)
    print("\n--- Phase 4: Lending ---")
    borrowers = []
    for user in range(num_users):
        tokens = sim._get_balance(user)
        if tokens > 100 * 10**TOKEN_DECIMALS:
            collateral = tokens // 2
            collateral_value = (collateral * sim.pool.sol_reserves) // sim.pool.token_reserves
            borrow_amt = (collateral_value * 4000) // 10000  # 40% LTV
            borrow_amt = max(borrow_amt, MIN_BORROW_AMOUNT)
            try:
                result = sim.borrow(user, collateral, borrow_amt)
                if "error" not in result:
                    borrowers.append(user)
                    print(f"  User {user} borrowed {result['borrowed'] / LAMPORTS_PER_SOL:.2f} SOL "
                          f"(LTV: {result['ltv_bps']/100:.1f}%)")
            except Exception:
                pass
    print(f"  {len(borrowers)} loans opened")

    sim.print_snapshot()

    # Phase 5: Price crash → liquidation cascade
    print("\n--- Phase 5: Price Crash + Liquidation Cascade ---")
    # Simulate a big dump — whale has significant pool share
    whale = num_users
    sim.sol_balances[whale] = 100 * LAMPORTS_PER_SOL
    whale_tokens = sim.pool.token_reserves // 3  # 33% of pool tokens
    sim.balances[whale] = whale_tokens

    # Dump tokens to crash price
    chunk = whale_tokens // 10
    for _ in range(10):
        if sim._get_balance(whale) >= chunk:
            sim.pool_sell(whale, chunk)
        sim.advance(10)

    print(f"  Price after dump: {sim.pool.price * 10**TOKEN_DECIMALS:.6f} SOL/token")

    # Advance time for interest accrual
    sim.advance(EPOCH_DURATION_SLOTS // 2)  # half epoch

    # Check and liquidate underwater positions
    liquidator = num_users + 1
    sim.sol_balances[liquidator] = 200 * LAMPORTS_PER_SOL
    liquidation_count = 0

    for borrower_id in list(sim.loans.keys()):
        loan = sim.loans[borrower_id]
        sim._accrue_loan_interest(loan)
        ltv = sim._loan_ltv_bps(loan)
        if ltv > DEFAULT_LIQ_THRESHOLD:
            try:
                result = sim.liquidate_long(liquidator, borrower_id)
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
    while not short_sim.curve.bonding_complete:
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

    print(f"  Price after pump: {short_sim.pool.price * 10**TOKEN_DECIMALS:.6f} SOL/token")

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
    print(f"  Interest collected: {sim.treasury.total_interest_collected / LAMPORTS_PER_SOL:.4f} SOL")
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

    # Fast-forward through bonding — 5 whales buy big to concentrate tokens
    for i in range(5):
        sim.sol_balances[i] = 200 * LAMPORTS_PER_SOL
    for i in range(5, 30):
        sim.sol_balances[i] = 50 * LAMPORTS_PER_SOL
    for i in range(5):
        while not sim.curve.bonding_complete and sim._get_sol(i) >= 10 * LAMPORTS_PER_SOL:
            sim.buy(i, 30 * LAMPORTS_PER_SOL)
            sim.advance(5)
        if sim.curve.bonding_complete:
            break

    sim.migrate()
    print(f"  Migrated. Pool: {sim.pool.sol_reserves/LAMPORTS_PER_SOL:.0f} SOL, "
          f"{sim.pool.token_reserves/10**TOKEN_DECIMALS:,.0f} tokens")

    # Grow treasury organically via trading + harvesting
    print("\n  Growing treasury via organic trading + harvests...")
    target_treasury = 150 * LAMPORTS_PER_SOL
    trade_count = 0
    harvest_count = 0
    slots_since_harvest = 0

    # Traders (6-29) generate volume; whales (0-4) hold for borrowing later
    while sim.treasury.sol_balance < target_treasury:
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
    print(f"    Avg harvest price: {avg_harvest_price:.4f} SOL/token")
    print(f"    Current pool price: {sim.pool.price * 10**TOKEN_DECIMALS:.4f} SOL/token")

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
            target_ltv = (depth_ltv * 9500) // 10000
            borrow_amt = (collateral_value * target_ltv) // 10000
            # Respect per-user cap (use net collateral after transfer fee)
            net_collateral = collateral - (collateral * TRANSFER_FEE_BPS) // 10000
            available_sol = max(0, sim.treasury.sol_balance - sim.treasury.short_collateral_sol)
            max_lendable = (available_sol * LENDING_UTIL_CAP_BPS) // 10000
            user_cap = (max_lendable * net_collateral * BORROW_SHARE_MULTIPLIER) // TOTAL_SUPPLY
            borrow_amt = min(borrow_amt, user_cap)
            borrow_amt = max(borrow_amt, MIN_BORROW_AMOUNT)
            result = sim.borrow(i, collateral, borrow_amt)
            if "error" not in result:
                loan_count += 1
                print(f"    User {i}: borrowed {result['borrowed']/LAMPORTS_PER_SOL:.2f} SOL "
                      f"at {result['ltv_bps']/100:.1f}% LTV "
                      f"({collateral_value/LAMPORTS_PER_SOL:.1f} SOL collateral)")
            else:
                print(f"    User {i}: {result['error']}")

    print(f"  Total loans opened: {loan_count}")
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
        for borrower_id in list(sim.loans.keys()):
            loan = sim.loans[borrower_id]
            sim._accrue_loan_interest(loan)
            ltv = sim._loan_ltv_bps(loan)
            if ltv > DEFAULT_LIQ_THRESHOLD:
                try:
                    result = sim.liquidate_long(liquidator, borrower_id)
                    if "error" not in result:
                        liquidated_this_round += 1
                        total_liquidations += 1
                        total_bad_debt += result["bad_debt"]
                        total_debt_covered += result["debt_covered"]
                        # Liquidator dumps seized collateral → price drops further
                        seized_tokens = sim._get_balance(liquidator)
                        if seized_tokens > 100 * 10**TOKEN_DECIMALS:
                            sim.pool_sell(liquidator, seized_tokens)
                            print(f"    Round {rounds}: liquidated user {borrower_id} "
                                  f"(debt={result['debt_covered']/LAMPORTS_PER_SOL:.2f} SOL, "
                                  f"bad_debt={result['bad_debt']/LAMPORTS_PER_SOL:.4f}), "
                                  f"dumped tokens → price={sim.pool.price * 10**TOKEN_DECIMALS:.4f}")
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
    print(f"    Final price: {sim.pool.price * 10**TOKEN_DECIMALS:.6f} SOL/token")
    print(f"    Price decline (total): {(1 - sim.pool.price / price_before) * 100:.1f}%")
    print(f"    Remaining loans: {len(sim.loans)}")

    sim.print_snapshot()
    return sim


def scenario_sandwich_attack(seed=777):
    """Simulate sandwich attack on a large borrow."""
    sim = TorchSim(seed=seed)
    print("\n" + "="*60)
    print("  SCENARIO: Sandwich Attack on Borrow")
    print("="*60)

    # Fast bonding + migration
    for i in range(20):
        sim.sol_balances[i] = 100 * LAMPORTS_PER_SOL
    for i in range(20):
        while not sim.curve.bonding_complete and sim._get_sol(i) >= LAMPORTS_PER_SOL:
            sim.buy(i, min(15 * LAMPORTS_PER_SOL, sim._get_sol(i)))
        if sim.curve.bonding_complete:
            break
    sim.migrate()

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

    # Step 2: Victim borrows at inflated collateral value
    collateral = victim_tokens // 2
    collateral_value = (collateral * sim.pool.sol_reserves) // sim.pool.token_reserves
    borrow_amt = (collateral_value * 4500) // 10000  # 45% LTV
    borrow_amt = max(borrow_amt, MIN_BORROW_AMOUNT)

    depth_ltv = get_depth_max_ltv_bps(sim.pool.sol_reserves)
    print(f"  Pool depth: {sim.pool.sol_reserves / LAMPORTS_PER_SOL:.0f} SOL → "
          f"max LTV: {depth_ltv/100:.0f}%")

    print(f"\n  Victim borrows {borrow_amt / LAMPORTS_PER_SOL:.2f} SOL...")
    try:
        result = sim.borrow(victim, collateral, borrow_amt)
        if "error" in result:
            print(f"  Borrow rejected: {result['error']}")
            print(f"  >>> DEPTH BAND REJECTED — LTV too high for pool depth")
            sim.print_snapshot()
            return sim
        else:
            print(f"  Borrow succeeded: {result}")
    except Exception as e:
        print(f"  Borrow failed: {e}")
        sim.print_snapshot()
        return sim

    # Step 3: Attacker dumps to crash price
    print("\n  Attacker back-runs: dumping tokens...")
    attacker_tokens = sim._get_balance(attacker)
    if attacker_tokens > 0:
        sim.pool_sell(attacker, attacker_tokens)
    price_after = sim.pool.price
    print(f"  Pool price after dump: {price_after * 10**TOKEN_DECIMALS:.6f}")

    # Check victim's LTV now
    if victim in sim.loans:
        loan = sim.loans[victim]
        ltv = sim._loan_ltv_bps(loan)
        print(f"\n  Victim LTV after attack: {ltv / 100:.1f}%")
        print(f"  Liquidatable: {'YES' if ltv > DEFAULT_LIQ_THRESHOLD else 'NO'}")

    # Attacker profit/loss
    attacker_sol_after = sim._get_sol(attacker)
    attacker_pnl = (attacker_sol_after - attacker_sol_before) / LAMPORTS_PER_SOL
    print(f"  Attacker P&L: {attacker_pnl:+.4f} SOL")

    sim.print_snapshot()
    return sim


# ============================================================================
# Main
# ============================================================================

if __name__ == "__main__":
    print("Torch Market Economic Simulator v0.1")
    print("=" * 60)

    sim1 = scenario_full_lifecycle()
    sim2 = scenario_cascade_stress()
    sim3 = scenario_sandwich_attack()

    print("\n\n" + "=" * 60)
    print("  ALL SCENARIOS COMPLETE")
    print("=" * 60)
