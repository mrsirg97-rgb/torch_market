"""Stress scenarios (added 2026-06-09, post prompt-002 review).

Three questions raised by the correctness review + the lending-model revision
(sticky gate, FCFS full-drain float — see memory: lending-capacity-model):

  1. short_death_spiral       — procyclicality: shorts drain pool SOL, which
                                shrinks the depth rails; does leverage stop
                                adding pressure before the pool collapses?
  2. full_utilization_baddebt — the old "0 bad debt to ~55% crash" was measured
                                at effective ~31% utilization; remeasure with
                                the float fully lent (FCFS, no util cap).
  3. sticky_gate_lifecycle    — economic mirror of the Kani sticky-gate proof:
                                unlock once → drain ≠ re-lock; only a bad-debt
                                write-off re-locks.
"""

from __future__ import annotations

from constants import *
from models import *
from engine import TorchSim
from scenarios.common import (
    _setup_migrated,
    _setup_migrated_depth,
    _warm_oracle,
)


def scenario_short_death_spiral(seed=4001):
    """Sustained short pressure + spot panic-selling on a mid pool.

    Each round: a fresh bear opens a max short (sale → pool SOL drains), and
    existing holders panic-sell a slice of their tokens. The depth rails should
    throttle NEW leverage (PoolTooThin below the 100-SOL floor) while spot
    selling continues — i.e. leverage must stop ADDING pressure before the pool
    collapses. Longs opened pre-spiral measure the cascade's bad-debt cost.

    PASS: (a) zero successful short opens at pool < DEPTH_FLOOR_SOL;
          (b) long bad debt ≤ the longs' own aggregate debt (custody bound);
          (c) SOL + token conservation throughout.
    """
    print("\n" + "=" * 60)
    print("  STRESS #4: Short-Pressure Death Spiral (procyclicality)")
    print("=" * 60)

    sim = _setup_migrated_depth(seed, 300 * LAMPORTS_PER_SOL)
    sim.treasury.sol_balance = 400 * LAMPORTS_PER_SOL  # gate cleared, longs live
    _warm_oracle(sim)

    # Two longs open before the spiral (the cascade victims).
    LONGS, LIQ = (60, 61), 80
    sim.sol_balances[LIQ] = 1_000_000 * LAMPORTS_PER_SOL
    long_debt_total = 0
    for u in LONGS:
        col = (30 * LAMPORTS_PER_SOL) * sim.pool.token_reserves // sim.pool.sol_reserves
        sim.balances[u] = col
        r = sim.open_leveraged_long(u, col)
        assert "error" not in r, f"long setup failed: {r}"
        long_debt_total += r["borrowed_sol_gross"]

    # Fund every bear wallet BEFORE the conservation snapshot (the snapshot
    # must cover all SOL that will ever enter the system).
    for bear in range(100, 131):
        sim.sol_balances[bear] = 100 * LAMPORTS_PER_SOL
    sol0, tok0 = sim._system_sol_total(), sim._system_token_total()

    print(f"\n  start: pool {sim.pool.sol_reserves/LAMPORTS_PER_SOL:.0f} SOL, "
          f"lock {sim.lock_tokens/10**12:.0f}M tokens, "
          f"longs debt {long_debt_total/LAMPORTS_PER_SOL:.1f} SOL")
    print(f"\n  {'round':>5} {'pool SOL':>9} {'maxLTV':>7} {'rail2':>8} {'lock M':>7} "
          f"{'short':>7} {'price drop':>11} {'liq?':>5}")

    p0 = sim.pool.price
    opens_below_floor = 0
    bad_debt = 0
    next_bear = 100  # fresh bear wallet per round
    for rnd in range(1, 31):
        pool_before_open = sim.pool.sol_reserves
        bear = next_bear
        next_bear += 1
        r = sim.open_short(bear, 50 * LAMPORTS_PER_SOL)
        opened = "error" not in r
        if opened and pool_before_open < DEPTH_FLOOR_SOL:
            opens_below_floor += 1  # leverage added pressure below the floor
        if not opened:
            ok_refusals = ("pool too thin", "short too small", "no tokens available")
            assert any(s in r["error"] for s in ok_refusals), \
                f"unexpected short refusal: {r['error']}"

        # Panic spot selling: three holders dump 3% of their balance each.
        for i in range(3):
            holder = (rnd * 3 + i) % 50
            bal = sim.balances.get(holder, 0)
            if bal > 1_000_000:
                sim.pool_sell(holder, bal * 3 // 100)

        # Liquidators chew on the longs as they breach.
        liq_hit = False
        for u in LONGS:
            if u in sim.longs:
                lr = sim.liquidate_leveraged_long(LIQ, u)
                if "error" not in lr:
                    bad_debt += lr["bad_debt"]
                    liq_hit = True
        sim.advance(400)

        drop = (1 - sim.pool.price / p0) * 100
        print(f"  {rnd:>5} {sim.pool.sol_reserves/LAMPORTS_PER_SOL:>8.1f} "
              f"{get_depth_max_ltv_bps(sim.pool.sol_reserves)/100:>6.0f}% "
              f"{max_debt_value_for_depth(sim.pool.sol_reserves)/LAMPORTS_PER_SOL:>7.1f} "
              f"{sim.lock_tokens/10**12:>6.0f} "
              f"{'open' if opened else 'REFUSE':>7} {drop:>10.1f}% "
              f"{'✓' if liq_hit else '':>5}")
        if sim.pool.sol_reserves < DEPTH_FLOOR_SOL // 2:
            break

    cons = (sim._system_sol_total() == sol0 and sim._system_token_total() == tok0)
    print(f"\n  leverage opens below the {DEPTH_FLOOR_SOL/LAMPORTS_PER_SOL:.0f}-SOL floor: "
          f"{opens_below_floor} (must be 0)")
    print(f"  long bad debt: {bad_debt/LAMPORTS_PER_SOL:.3f} SOL "
          f"(bound: their own debt {long_debt_total/LAMPORTS_PER_SOL:.1f} SOL)")
    print(f"  conservation: {'✓' if cons else '✗'}")

    assert opens_below_floor == 0, "leverage added pressure below the depth floor"
    assert bad_debt <= long_debt_total, "bad debt exceeded the custody bound"
    assert cons, "conservation violated"
    print("\n  Read: spot selling can always continue (that's the market), but the")
    print("  rails throttle NEW leverage to zero before the pool collapses — the")
    print("  spiral's leveraged component is self-limiting; losses stay custody-bounded.")
    return sim


def scenario_full_utilization_baddebt(seed=4002):
    """Bad-debt severity with the float FULLY lent (FCFS, no utilization cap).

    The pre-revision measurement ('0 bad debt to ~55% crash') ran at effective
    ~31% utilization (the double-counted formula). Here borrowers exhaust the
    float (per-user 20% cap each), then a whale dump sweep crashes the pool;
    liquidators resolve everything liquidatable.

    PASS: total bad debt ≤ total lent (custody bound) at every severity;
    conservation holds.
    """
    print("\n" + "=" * 60)
    print("  STRESS #5: Bad Debt at FULL Utilization (FCFS float)")
    print("=" * 60)
    print(f"\n  {'dump (xpool)':>13} {'crash %':>9} {'lent SOL':>9} {'util %':>7} "
          f"{'bad debt':>11} {'loss %':>7} {'cons':>5}")

    worst_loss_pct = 0.0
    for mult in (0.5, 1.0, 2.0, 4.0, 8.0):
        sim = _setup_migrated_depth(seed, 500 * LAMPORTS_PER_SOL)
        sim.treasury.sol_balance = 200 * LAMPORTS_PER_SOL
        _warm_oracle(sim)
        LIQ, WHALE = 80, 81
        sim.sol_balances[LIQ] = 1_000_000 * LAMPORTS_PER_SOL
        sim.balances[WHALE] = int(12 * sim.pool.token_reserves)

        # Exhaust the float: borrowers each take their 20% cap until refused.
        borrowers = []
        for u in range(60, 75):
            col = (60 * LAMPORTS_PER_SOL) * sim.pool.token_reserves // sim.pool.sol_reserves
            sim.balances[u] = col
            r = sim.open_leveraged_long(u, col)
            if "error" in r:
                break
            borrowers.append(u)
        lent = sim.treasury.total_sol_lent_to_longs
        assets_before = sim.treasury.lending_assets
        util = sim.treasury.utilization_bps / 100
        sol0, tok0 = sim._system_sol_total(), sim._system_token_total()

        p0 = sim.pool.price
        sim.pool_sell(WHALE, int(mult * sim.pool.token_reserves))
        crash = (1 - sim.pool.price / p0) * 100

        bad_debt = 0
        for _ in range(400):
            progressed = False
            for u in list(borrowers):
                if u in sim.longs:
                    lr = sim.liquidate_leveraged_long(LIQ, u)
                    if "error" not in lr:
                        bad_debt += lr["bad_debt"]
                        progressed = True
            if not progressed:
                break

        cons = (sim._system_sol_total() == sol0 and sim._system_token_total() == tok0)
        loss_pct = bad_debt * 100.0 / max(1, assets_before)
        worst_loss_pct = max(worst_loss_pct, loss_pct)
        assert bad_debt <= lent, "bad debt exceeded total lent (custody bound broken)"
        assert cons, "conservation violated"
        print(f"  {mult:>13.1f} {crash:>8.1f}% {lent/LAMPORTS_PER_SOL:>8.1f} "
              f"{util:>6.0f}% {bad_debt/LAMPORTS_PER_SOL:>10.3f} {loss_pct:>6.1f}% "
              f"{'✓' if cons else '✗':>5}")

    print(f"\n  Read: worst principal loss {worst_loss_pct:.1f}% across the sweep —")
    print("  full utilization scales the dollar loss with the float, but each")
    print("  position's loss stays bounded by its own vault (rail-2 sizing +")
    print("  per-user caps spread the book), and write-offs re-lock the gate")
    print("  exactly when the principal drops below the unlock bar.")
    return None


def scenario_sticky_gate_lifecycle(seed=4003):
    """Economic walk of the sticky unlock gate (mirror of the Kani proof
    verify_lending_gate_sticky_under_borrow + the FCFS capacity model):

      1. principal < MIN            → open refused (lending floor)
      2. principal topped to MIN+   → open succeeds
      3. float drained by borrows   → gate STAYS open; refusal is float-
                                      exhausted, NOT the lending floor
      4. repay                      → capacity restored, no re-unlock needed
      5. crash → bad-debt write-off → principal drops below MIN → gate
                                      RE-LOCKED (lending floor again)
    """
    print("\n" + "=" * 60)
    print("  STRESS #6: Sticky Unlock Gate Lifecycle")
    print("=" * 60)

    sim = _setup_migrated_depth(seed, 600 * LAMPORTS_PER_SOL)
    _warm_oracle(sim)
    B1, B2, LIQ, WHALE = 60, 61, 80, 81
    sim.sol_balances[LIQ] = 1_000_000 * LAMPORTS_PER_SOL
    sim.balances[WHALE] = int(12 * sim.pool.token_reserves)
    col = (90 * LAMPORTS_PER_SOL) * sim.pool.token_reserves // sim.pool.sol_reserves
    for u in (B1, B2):
        sim.balances[u] = col

    # 1. Below the bar → lending floor.
    sim.treasury.sol_balance = MIN_TREASURY_SOL_FOR_LENDING - LAMPORTS_PER_SOL
    r = sim.open_leveraged_long(B1, col)
    assert "error" in r and "lending floor" in r["error"], f"step 1: {r}"
    print(f"  1. principal {sim.treasury.lending_assets/LAMPORTS_PER_SOL:.0f} SOL < bar "
          f"→ refused (lending floor) ✓")

    # 2. Top to the bar → opens.
    sim.treasury.sol_balance = MIN_TREASURY_SOL_FOR_LENDING + 5 * LAMPORTS_PER_SOL
    r1 = sim.open_leveraged_long(B1, col)
    assert "error" not in r1, f"step 2: {r1}"
    print(f"  2. principal at bar → open OK "
          f"(borrowed {r1['borrowed_sol_gross']/LAMPORTS_PER_SOL:.1f} SOL) ✓")

    # 3. Drain the float (more borrowers), then: gate open, float exhausted.
    for u in range(62, 75):
        sim.balances[u] = col
        if "error" in sim.open_leveraged_long(u, col):
            break
    sim.sol_balances[B2] = 0
    r = sim.open_leveraged_long(B2, col)
    assert "error" in r, f"step 3 expected refusal: {r}"
    assert "lending floor" not in r["error"], \
        f"step 3: gate flapped (re-locked on drain): {r['error']}"
    print(f"  3. float drained (float {sim.treasury.available_to_lend/LAMPORTS_PER_SOL:.2f} SOL, "
          f"principal {sim.treasury.lending_assets/LAMPORTS_PER_SOL:.0f} SOL) → "
          f"refusal is '{r['error'][:42]}…', gate STILL OPEN ✓")

    # 4. Repay → capacity restored without any re-unlock.
    sim.close_leveraged_long(B1, 10000)
    r2 = sim.open_leveraged_long(B2, col)
    assert "error" not in r2, f"step 4: {r2}"
    print(f"  4. repay restored capacity → open OK "
          f"(borrowed {r2['borrowed_sol_gross']/LAMPORTS_PER_SOL:.1f} SOL) ✓")

    # 5. Crash → bad-debt write-off shrinks the principal below the bar → re-lock.
    assets_before = sim.treasury.lending_assets
    sim.pool_sell(WHALE, int(8 * sim.pool.token_reserves))
    bad_debt = 0
    for _ in range(400):
        progressed = False
        for u in list(sim.longs.keys()):
            lr = sim.liquidate_leveraged_long(LIQ, u)
            if "error" not in lr:
                bad_debt += lr["bad_debt"]
                progressed = True
        if not progressed:
            break
    assets_after = sim.treasury.lending_assets
    print(f"  5. crash: bad debt {bad_debt/LAMPORTS_PER_SOL:.1f} SOL wrote the principal "
          f"{assets_before/LAMPORTS_PER_SOL:.0f} → {assets_after/LAMPORTS_PER_SOL:.0f} SOL")
    if assets_after < MIN_TREASURY_SOL_FOR_LENDING:
        sim.balances[B1] = col
        r = sim.open_leveraged_long(B1, col)
        assert "error" in r and "lending floor" in r["error"], f"step 5: {r}"
        print("     principal below bar → gate RE-LOCKED (lending floor) ✓")
    else:
        print("     (write-off did not breach the bar at this severity — gate stays")
        print("      open, consistent: re-lock requires loss > principal − bar)")

    print("\n  Read: the gate is a one-time earnings milestone — borrowing can never")
    print("  flap it; only a REAL loss (bad debt) can take it back. Matches the Kani")
    print("  sticky-gate proof and V20's documented sol_balance semantics.")
    return sim
