"""TorchSim — the protocol state machine (economic spec for the on-chain program)."""
# Extracted from the former monolithic torch_sim.py (split 2026-06-09).
# torch_sim.py remains the entry point / public surface.

from __future__ import annotations
import random
import math
from dataclasses import dataclass, field
from typing import Optional

from constants import *
from models import *

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

        # [F-5] Borrow plan — ALL clamps BEFORE the fee, so the fee prices the
        # capacity actually consumed (mirrors the long side's post-clamp fee):
        #   borrow_value = (collateral * ltv / 10000).min(rail2 size cap)
        #   tokens = (borrow_value → tokens).min(lock).min(MAX_WALLET_TOKENS)
        #   realized_borrow_value = tokens → SOL at pre-swap price
        #   open_fee = realized * OPEN_FEE_BPS / 10000  (SOL → treasury)
        #   net_collateral = collateral - open_fee
        borrow_value_sol = (sol * effective_max_ltv) // 10000
        # [size cap] keep the position's SOL-debt-value ≤ ρ_max of pool depth so
        # the liquidation unwind slippage stays bounded regardless of pool size.
        size_cap = max_debt_value_for_depth(self.pool.sol_reserves)
        if borrow_value_sol > size_cap:
            borrow_value_sol = size_cap
        tokens_to_borrow = (borrow_value_sol * self.pool.token_reserves) \
                           // max(1, self.pool.sol_reserves)

        # Lock availability + per-user cap (MAX_WALLET_TOKENS). lock_tokens is
        # the physical lock balance — tokens already lent out have physically
        # left it, so it IS the lendable amount; full-drain by design (the real
        # aggregate brake is pool depth: each open drains pool SOL → rails shrink).
        available_in_lock = self.lock_tokens
        tokens_to_borrow = min(tokens_to_borrow, available_in_lock)
        tokens_to_borrow = min(tokens_to_borrow, MAX_WALLET_TOKENS)
        if tokens_to_borrow <= 0:
            self._credit_sol(user_id, sol)
            return {"error": "no tokens available (per-user cap or lock empty)"}
        if tokens_to_borrow < MIN_SHORT_TOKENS:
            self._credit_sol(user_id, sol)
            return {"error": f"short too small ({tokens_to_borrow} < {MIN_SHORT_TOKENS})"}

        # [F-5] Fee on the REALIZED (post-clamp) borrow value.
        realized_borrow_value = (tokens_to_borrow * self.pool.sol_reserves) \
                                // max(1, self.pool.token_reserves)
        open_fee = (realized_borrow_value * OPEN_FEE_BPS) // 10000
        net_collateral = sol - open_fee

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
        # [F-2] STICKY unlock gate on the principal pool (physical float + lent):
        # invariant under borrow/repay, so once earned the gate stays open; only
        # a bad-debt write-off re-locks it. Capacity below is the physical float.
        if self.treasury.lending_assets < MIN_TREASURY_SOL_FOR_LENDING:
            return {"error": f"lending floor (need {MIN_TREASURY_SOL_FOR_LENDING}, "
                              f"principal {self.treasury.lending_assets})"}
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

        # [F-1][F-3] Per-user cap = min(formula, absolute) on the principal
        # pool, measured on the position's collateral (sim is 1 position/user,
        # so per-position == per-user aggregate — the UserRisk equivalence).
        max_lendable = self.treasury.lending_assets
        formula_cap = (max_lendable * net_collateral * BORROW_SHARE_MULTIPLIER) \
                      // TOTAL_SUPPLY
        absolute_cap = (max_lendable * MAX_USER_BORROW_SHARE_BPS) // 10000
        user_cap = min(formula_cap, absolute_cap)
        # [F-2] Global headroom = the physical float (FCFS, full-drain).
        global_headroom = self.treasury.available_to_lend
        # [size cap] keep the position's SOL-debt-value ≤ ρ_max of pool depth so
        # the liquidation unwind slippage stays bounded regardless of pool size.
        size_cap = max_debt_value_for_depth(self.pool.sol_reserves)
        clamped = min(desired_borrow_sol, user_cap, global_headroom, size_cap)
        if clamped != desired_borrow_sol:
            desired_borrow_sol = clamped
            open_fee = (desired_borrow_sol * OPEN_FEE_BPS) // 10000
            atomic_buy_sol = desired_borrow_sol - open_fee
        if global_headroom < MIN_BORROW_AMOUNT:
            self._credit_tokens(user_id, net_collateral)
            return {"error": "global lending cap exceeded (float exhausted)"}
        if desired_borrow_sol < MIN_BORROW_AMOUNT:
            self._credit_tokens(user_id, net_collateral)
            return {"error": f"desired_borrow {desired_borrow_sol} "
                              f"below MIN_BORROW_AMOUNT"}

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
