"""Adversarial scenarios: MEV, gaming, oracle manipulation, solvency gaps."""
# Extracted from the former monolithic torch_sim.py (split 2026-06-09).
# torch_sim.py remains the entry point / public surface.

from __future__ import annotations
import random
import math

from constants import *
from models import *
from engine import TorchSim
from scenarios.common import (
    _setup_migrated,
    _setup_migrated_depth,
    _warm_oracle,
    _run_manipulation_attack,
    _setup_twap_short_at_liquidation,
)

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
