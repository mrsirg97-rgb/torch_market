# torch_market Property Testing Report

## Overview

torch_market's core arithmetic is property-tested using [proptest](https://proptest-rs.github.io/proptest/), a Rust fuzz-style property testing framework. Properties cover fee calculations, rate curves, bonding-curve swap math, lending and liquidation math, protocol reward distribution, migration math, and short-selling math.

Proptest complements the Kani harnesses ([verification.md](./verification.md)): Kani exhaustively proves exact correctness at concrete representative values, proptest explores the full u64 input space with thousands of randomly-drawn cases per property and automatically shrinks any failing input down to the minimal reproducing case.

**Tool:** proptest 1.x
**Target:** `torch_market` v20.0.0 (torch_next)
**Properties:** 31 properties across 7 modules, all passing
**Cases per property:** 5,000
**Total assertions per run:** ~155,000
**Source:** `programs/torch_market/tests/math_proptests.rs`
**Run with:** `cargo test -p torch_market --test math_proptests`

Located under `tests/` rather than `src/` so the `proptest!` macro DSL isn't parsed by Anchor's `#[program]` safety-check macro, which walks the lib source tree with syn and doesn't understand macro semantics.

## What Is Verified

### Fees (Properties 1-7)

| Property | Description |
|----------|-------------|
| `protocol_fee_bounded` | `calc_protocol_fee(sol, bps) ≤ sol` for all valid bps |
| `protocol_fee_monotonic` | Larger input → larger fee at fixed rate |
| `dev_share_bounded_by_input` | Dev wallet share ≤ total |
| `token_treasury_fee_bounded` | Token treasury fee ≤ input |
| `creator_fee_share_bounded` | Creator fee share ≤ input |
| `transfer_fee_bounded` | Token-2022 transfer fee ≤ `amount + 1` and ≤ `MAX_TRANSFER_FEE` ceiling |
| `transfer_fee_ceiling` | When fee is below the cap, it's computed exactly as `amount × TRANSFER_FEE_BPS / 10_000` (no rounding drift down) |

### Rate Curves — Treasury Decay, Creator Growth (Properties 8-11)

| Property | Description |
|----------|-------------|
| `treasury_rate_within_bounds` | Treasury rate stays between `TREASURY_SOL_MIN_BPS` and `TREASURY_SOL_MAX_BPS` across the full bonding range |
| `treasury_rate_monotonic_decreasing` | Treasury rate only decreases as bonding progresses |
| `creator_rate_within_bounds` | Creator rate stays between `CREATOR_SOL_MIN_BPS` and `CREATOR_SOL_MAX_BPS` |
| `creator_rate_monotonic_increasing` | Creator rate only increases as bonding progresses |

Rate curves are tested for both `BONDING_TARGET_FLAME` and `BONDING_TARGET_TORCH` targets.

### Bonding Curve Swap (Properties 12-17)

| Property | Description |
|----------|-------------|
| `tokens_out_bounded_by_vt` | Buy output `< virtual_token_reserve`; strict inequality when input > 0 |
| `tokens_out_zero_input_is_zero` | Zero SOL in → zero tokens out |
| `tokens_out_monotonic` | Larger input → ≥ output |
| `bonding_curve_k_non_decreasing` | `K_after ≥ K_before` on every bonding-curve buy — self-deepening property |
| `sol_out_bounded_by_vs` | Sell output `< virtual_sol_reserve` when tokens > 0 |
| `sol_out_zero_input_is_zero` | Zero tokens in → zero SOL out |

### Lending — Collateral Value, LTV, Interest, Liquidation (Properties 18-24)

| Property | Description |
|----------|-------------|
| `collateral_value_zero_collateral_is_zero` | Empty collateral → zero value |
| `collateral_value_monotonic_in_collateral` | Larger collateral → ≥ value at fixed pool state |
| `ltv_zero_collateral_is_max` | LTV with zero collateral = `u64::MAX` (saturating semantics) |
| `ltv_zero_debt_is_zero` | Zero debt → zero LTV |
| `interest_monotonic_in_principal` | Interest accrual ≥ when principal larger, at fixed rate and slots |
| `interest_monotonic_in_slots` | Interest accrual ≥ when more slots elapsed, at fixed rate and principal |
| `collateral_to_seize_monotonic_in_debt` | Larger debt to liquidate → ≥ collateral seized |

### Protocol Rewards (Properties 25-27)

| Property | Description |
|----------|-------------|
| `user_share_bounded_by_distributable` | Any user's share ≤ total distributable (conservation) |
| `claim_with_cap_respects_cap` | Claim ≤ `MAX_CLAIM_SHARE_BPS × distributable / 10_000` (anti-monopoly cap) |
| `claim_monopoly_trader_hits_cap` | A monopoly trader (user_volume == total_volume) claims exactly the cap, not more |

### Migration (Property 28)

| Property | Description |
|----------|-------------|
| `tokens_for_pool_cross_multiply` | Floor-division property: `tokens_for_pool × virtual_sol ≤ real_sol × virtual_tokens` with residual `< virtual_sol` — proves the migration token allocation rounds down, never up |

### Short Selling (Properties 29-31)

| Property | Description |
|----------|-------------|
| `short_debt_value_bounded_when_debt_le_reserve` | When token debt ≤ pool tokens, valued-in-SOL debt ≤ pool SOL (can't value debt above pool liquidity) |
| `short_interest_monotonic_in_tokens` | Short interest accrues monotonically in token debt at fixed rate/slots |
| `short_sol_to_seize_grossed_up_by_bonus` | Liquidation seize amount is between `debt × 1.0` and `debt × (1 + bonus_bps)` — bonus is applied as a ceiling, not a floor |

## Why Proptest Alongside Kani

The two tools cover different ground:

**Kani (model checking)** — Exhaustive proofs at *concrete* representative values (and symbolic for simple u64 arithmetic). Proves exact correctness for the tested inputs. Struggles with free u64 symbolic inputs flowing through u128 multiply/divide chains — SAT solvers can't handle that efficiently.

**Proptest (property-based fuzzing)** — Randomly samples the input space with automatic shrinking. 5,000 distinct cases per property catches violations that concrete-value testing may miss. Doesn't prove exhaustive correctness but greatly widens empirical coverage, especially for the broad bounds on multi-argument functions.

Together: Kani pins down behavior at edges and representative points; proptest sweeps the middle and catches anything that slips between Kani's concrete probes.

**Regression durability:** proptest automatically writes failing seeds to `proptest-regressions/` on failure and replays them on every subsequent run. Any future regression on a property that fired in the past is caught deterministically.

## What Is NOT Verified

The same exclusions as Kani — neither tool covers:

- Access control (account constraints, PDA ownership, Signer checks)
- CPI safety (DeepPool interaction edge cases, Token-2022 transfer hook reentrancy)
- Economic attacks (sandwich, front-running, MEV) — see [audit.md](./audit.md) and `sim/torch_sim.py` for adversarial coverage
- Rent-exempt minimum handling edge cases
- Network-level concerns (transaction ordering, commitment levels)
- The `vault_sol` / `TorchVault` split surface — covered by the code audit (see [audit.md](./audit.md) §V20.0.0 Refinements and Deep Dive #7)

These require code audit and adversarial testing.

## Constants Used in Ranges

| Constant | Value | Description |
|----------|-------|-------------|
| `CASES` | 5,000 | Randomly-drawn inputs per property |
| `REALISTIC_MAX` | 10^18 | Upper bound for composite properties — keeps u128 intermediaries safe |
| `PROTOCOL_FEE_BPS` | — | Protocol fee in bps (defined in constants.rs) |
| `TRANSFER_FEE_BPS` | — | Token-2022 transfer fee in bps |
| `MAX_TRANSFER_FEE` | — | Ceiling on absolute transfer fee |
| `MAX_CLAIM_SHARE_BPS` | — | Anti-monopoly cap on per-user reward share |
| `TREASURY_SOL_MIN_BPS` / `TREASURY_SOL_MAX_BPS` | — | Treasury rate bounds |
| `CREATOR_SOL_MIN_BPS` / `CREATOR_SOL_MAX_BPS` | — | Creator rate bounds |
| `BONDING_TARGET_FLAME` / `BONDING_TARGET_TORCH` | — | Two bonding completion targets |
| `EPOCH_DURATION_SLOTS` | — | Slot count per reward epoch |
