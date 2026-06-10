"""Shared scenario fixtures + attack helpers."""
# Extracted from the former monolithic torch_sim.py (split 2026-06-09).
# torch_sim.py remains the entry point / public surface.

from __future__ import annotations
import random
import math

from constants import *
from models import *
from engine import TorchSim

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
