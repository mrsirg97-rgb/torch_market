"""
Torch Market Economic Simulator (V21)

Pure-Python simulation of the V21 Torch Market protocol:
  - Bonding curve (constant product with dynamic fee splits)
  - Post-migration DeepPool (immutable CPMM + keeperless TWAP oracle)
  - V21 atomic-custodied leveraged long (SOL borrow → atomic pool_buy → vault tokens)
  - V21 atomic-custodied short (token borrow → atomic pool_sell → vault SOL)
  - Liquidation cascades (per-position vault seize)
  - Transfer fee harvesting
  - Open fee (0.5% of REALIZED borrow value in SOL → treasury)
  - Sticky lending unlock gate (principal pool) + FCFS float capacity

All math mirrors the on-chain Rust (integer arithmetic, checked ops).
No external deps — stdlib only.

This file is the ENTRY POINT + public surface; the implementation lives in:
  constants.py            mirror of constants.rs (+ depth-rail fns)
  models.py               BondingCurve, Pool (+oracle), Treasury, positions, log
  engine.py               TorchSim state machine (the economic spec)
  scenarios/lifecycle.py  full lifecycle, cascade, closure, invariant sweep
  scenarios/adversarial.py  MEV, gaming, oracle manipulation, solvency gaps
  scenarios/leverage.py   leverage lifecycle + PnL + bad-debt severity
  scenarios/risk_rails.py TWAP window, bonus ramp, depth rails, carry
  scenarios/stress.py     death spiral, full-utilization bad debt, sticky gate

Run all scenarios:  python sim/torch_sim.py

Modeling notes (deliberate abstractions):
  - One position per (user, side). On-chain allows multiple via position_index,
    but the per-user caps aggregate across indices (UserRisk), so a single
    position per user is exactly equivalent under the caps.
  - Protocol-treasury epoch rewards (claim_protocol_rewards) are NOT modeled —
    the sim's domain is per-token economics; rewards are protocol-level and
    order-independent by construction (epoch_distributable_snapshot).
  - Close-path slippage args (min_surplus_sol_out) are not modeled — sim steps
    are atomic, so there is no quote/execution race to guard.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from constants import *  # noqa: F401,F403
from models import *  # noqa: F401,F403
from engine import TorchSim  # noqa: F401

from scenarios.lifecycle import (  # noqa: F401
    scenario_full_lifecycle,
    scenario_cascade_stress,
    scenario_per_token_closure,
    scenario_invariant_check,
)
from scenarios.adversarial import (  # noqa: F401
    scenario_sandwich_attack,
    scenario_harvest_gaming,
    scenario_oracle_attack_sweep,
    scenario_oracle_liquidation_attack,
    scenario_solvency_gap_crash,
    scenario_oracle_mitigation_test,
)
from scenarios.leverage import (  # noqa: F401
    scenario_long_short_dump,
    scenario_leveraged_long,
    scenario_long_short_basis,
    scenario_directional_profitability,
    scenario_long_baddebt_severity,
)
from scenarios.risk_rails import (  # noqa: F401
    scenario_twap_tuning,
    scenario_liq_bonus_ramp,
    scenario_depth_rails,
    scenario_short_carry,
    scenario_short_breakeven,
)
from scenarios.stress import (  # noqa: F401
    scenario_short_death_spiral,
    scenario_full_utilization_baddebt,
    scenario_sticky_gate_lifecycle,
)

if __name__ == "__main__":
    print("Torch Market Economic Simulator v0.2 (modular)")
    print("=" * 60)

    scenario_full_lifecycle()
    scenario_cascade_stress()
    scenario_sandwich_attack()
    scenario_harvest_gaming()
    scenario_long_short_dump()
    scenario_leveraged_long()
    scenario_long_short_basis()
    scenario_directional_profitability()
    scenario_invariant_check()
    scenario_per_token_closure()
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
    scenario_short_death_spiral()
    scenario_full_utilization_baddebt()
    scenario_sticky_gate_lifecycle()

    print("\n\n" + "=" * 60)
    print("  ALL SCENARIOS COMPLETE")
    print("=" * 60)
