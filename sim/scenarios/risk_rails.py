"""Risk-rail tuning scenarios: TWAP window, bonus ramp, depth rails, carry."""
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
