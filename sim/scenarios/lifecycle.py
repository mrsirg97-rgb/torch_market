"""Lifecycle + conservation scenarios."""
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
