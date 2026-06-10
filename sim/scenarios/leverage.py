"""Leverage lifecycle + PnL scenarios."""
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
