# Depth-Scaled Risk Rails (LTV / Size / Bonus)

Not implemented. Design captured for execution. **The liquidation logic does not
change — only the three pure functions that produce its parameters do.** This is a
math swap, not a control-flow swap (see "What changes vs. what doesn't").

## Problem

The leverage system has three depth-dependent risks and currently controls them
with two coarse, mis-keyed knobs:

1. **Mark manipulation** — gated by a 4-step LTV ladder (`<50 SOL → 25%`,
   `50–200 → 35%`, `200–500 → 45%`, `500+ → 50%`). Step functions have cliff
   edges: one SOL of pool depth can flip a borrower a whole LTV tier, and the
   breakpoints (50/200/500) are arbitrary.
2. **Unwind slippage at liquidation** — **not gated at all.** The depth ladder
   caps *leverage* (LTV), not *position size relative to the pool*. A whale with
   large collateral can open a position worth 50% of a thin pool at 25% LTV, and
   nothing stops it.
3. **Liquidator incentive** — a flat `max_bonus = 10%`, ramping `0% → 10%` over
   `LTV 65% → 90%`.

The flat bonus is the binding hole. A liquidator acts iff `bonus ≥ unwind_slippage`,
and on a CPMM the slippage of round-tripping a position is **ρ = covered_debt / pool_SOL**.
The flat 10% only clears positions with **ρ ≲ 10%**; past that, no liquidator acts
anywhere in the `65→91%` band and the position rots straight to bad debt. With no
size cap, ρ is unbounded — so the protocol's bad-debt exposure is unbounded on thin
pools, regardless of how conservative the LTV ladder looks.

## The deeper design property

The three risks are **orthogonal** and each wants a **different** variable:

| Risk | Driven by | Right rail |
|---|---|---|
| Mark manipulation | pool depth `S` (cost to push the TWAP ∝ `S`) | **Max LTV**, a function of `S` |
| Unwind slippage | size *relative* to depth, `ρ = debt/S` | **Size cap**, `debt ≤ ρ_max·S` |
| Liquidator incentive | `ρ` (what slippage must be beaten) | **Bonus**, sized to `ρ_max` |

Today the ladder tries to make the LTV knob do double duty (manipulation *and*,
implicitly, sizing) and it fails at sizing, because **LTV is leverage, not size.**
Separate the three and each becomes a clean one-line function. The payoff:
**once `ρ` is capped, worst-case unwind slippage is `ρ_max` on *every* pool — it
becomes depth-invariant — so the bonus collapses back to a single flat constant.**

## The math

### Rail 1 — Max LTV: continuous, concave in depth

```
LTV(S) = LTV_max − (LTV_max − LTV_min) · (S_floor / S)^α
```

Inputs: **lower bound** `S_floor` (smallest pool we offer post-migration) with
`LTV_min` there; **current depth** `S`; **scaling factor** `α`. `LTV_max` is the
asymptote. Self-anchoring (no tier edges, no separate saturation point), monotonic,
**concave** — which is correct, because manipulation cost grows ~linearly with depth,
so the *marginal* safety bought per SOL shrinks as a fraction (the 101st SOL matters
more than the 1001st).

Reference values `LTV_min=30%`, `LTV_max=60%`, `S_floor=100 SOL`, `α=1`:

| pool `S` | LTV |
|---|---|
| 100 | 30.0% |
| 150 | 40.0% |
| 200 | 45.0% |
| 300 | 50.0% |
| 500 | 54.0% |
| 1000 | 57.0% |
| ∞ | 60.0% |

`α` is the conservatism dial (`α<1` keeps mid-size pools lower for longer; `α>1`
rushes the cap). At `S=200`: `α=0.5→39%`, `α=1→45%`, `α=2→52%`. Default `α=1`.

### Rail 2 — Size cap: linear in depth (the missing rail)

```
debt_value ≤ ρ_max · S            (reference ρ_max = 25%)
```

Allowed position grows linearly with the pool. This is the rail that makes unwind
slippage depth-invariant: capped at `ρ_max` regardless of `S`. Enforced **at open**
(reject borrows that would push the position's SOL-debt-value past `ρ_max·S`).

### Rail 3 — Bonus: flat, coupled to the insolvency line

The liquidator break-even is `bonus ≥ ρ`. With `ρ ≤ ρ_max` guaranteed by Rail 2,
size the bonus once:

```
max_bonus     ≈ 1.3 · ρ_max               (1.3 = liquidator margin over slippage)
full_bonus_ltv = 100% / (1 + max_bonus)   (ramp completes AT insolvency, not a fixed 90%)
```

For `ρ_max = 25%`: `max_bonus ≈ 33%`, `full_bonus_ltv ≈ 75%`. The bonus still ramps
`0%` at the 65% threshold → `max_bonus` at `full_bonus_ltv`; only the ceiling and the
completion point change. **No `bonus(S)` curve is needed** — depth-awareness lives
entirely in Rail 2's `ρ_max·S`.

### Why the bonus must couple to `full_bonus_ltv`

The seize is `debt × (1 + bonus)`, funded from the vault, so the position is solvent
only while `LTV × (1 + bonus) ≤ 100%` → **insolvency at `LTV = 100/(1+bonus)`**.
Raising the bonus *lowers* the LTV at which bad debt begins. A clean clear needs an
`L*` with `bonus(L*) ≥ ρ` **and** `L*·(1+bonus) ≤ 100`, so the ramp must reach
`max_bonus` *by* `100/(1+max_bonus)`, not at a hard-coded 90%. (Today's `90%` is only
self-consistent for `max_bonus ≈ 10%`: `90 × 1.10 = 99 ≤ 100`. Any larger bonus with a
fixed 90% completion would let positions cross insolvency before the bonus matures.)

### Break-even table (why flat-10% is the hole)

`ρ = covered_debt / pool_SOL`; flat ramp clears where `65 + 250ρ ≤ 90.9` → `ρ ≤ 10.4%`.

| ρ | needed bonus (≈1.3ρ) | insolvency LTV `100/(1+bonus)` | flat 10% | depth-rail (capped ρ + flat 33%) |
|---|---|---|---|---|
| 3% | 4% | 96% | clears ✓ | clears ✓ |
| 8% | 10% | 91% | edge | ✓ |
| 12% | 16% | 86% | **bad debt** | ✓ |
| 20% | 26% | 79% | **bad debt** | ✓ |
| 25% (`ρ_max`) | 33% | 75% | **bad debt** | clears ~75% ✓ |
| >40% | >53% | ≤65% | bad debt | **prevented by Rail 2** |

Past `ρ ≈ 40%` the insolvency line falls to the 65% trigger — the position is
insolvent the instant it's liquidatable and **no bonus can save it.** Rail 2's cap
exists precisely to keep ρ out of that region.

## What changes vs. what doesn't

**Unchanged (byte-for-byte the same control flow):**
- The liquidation trigger: `twap_ltv > threshold`, asymmetric spot veto, fail-closed
  on TWAP warmup.
- The OTC seize structure, the partial-close factor, the insolvent-residual
  forgiveness, the gross-up-for-fee handling.
- The bonus *ramp shape* (linear from 0 at the threshold).
- The 65% threshold itself — **the borrower's liquidation point stays contractual
  and flat.** Depth never moves the trigger; it only sets how much leverage you're
  granted (Rail 1), how big a position you may open (Rail 2), and the exit cost
  (Rail 3).

**Changed (three pure parameter functions + one new open-time check):**
1. `effective_max_ltv(S)` — step ladder → the concave formula.
2. `max_position_debt(S) = ρ_max · S` — **new** check at open.
3. `effective_liq_bonus_bps(...)` — `max_bonus` and `full_bonus_ltv` become derived
   (`1.3·ρ_max`, `100/(1+max_bonus)`) instead of the constants `1000` / `9000`.

Because the *flow* is untouched, the existing liquidation Kani harnesses and litesvm
flow tests stay valid; only the three parameter functions need fresh property tests
(monotonicity, anchoring, the `bonus`↔`full_bonus_ltv` coupling invariant).

## Implementation outline

- **`constants.rs`** — replace `DEPTH_TIER_*` / `DEPTH_LTV_*` and the flat
  `DEFAULT_LIQUIDATION_BONUS_BPS=1000` / `LIQ_FULL_BONUS_LTV_BPS=9000` with the
  curve parameters: `LTV_MIN_BPS`, `LTV_MAX_BPS`, `DEPTH_FLOOR_LAMPORTS` (100 SOL),
  `LTV_ALPHA` (fixed-point), `RHO_MAX_BPS`, `BONUS_SAFETY_BPS` (1.3×).
- **`math.rs`** — three pure fns: `depth_max_ltv_bps(S)`, `max_debt_for_depth(S)`,
  and `derived_bonus(rho_max)` → `(max_bonus_bps, full_bonus_ltv_bps)`. Keep
  `effective_liq_bonus_bps` but feed it the derived pair. (Fixed-point `^α` /
  reciprocal needs an integer-safe implementation — `α=1` is just
  `LTV_max − range·S_floor/S`, division-only, Kani-friendly; non-unit `α` needs a
  fixed-point pow or a lookup table.)
- **Open handlers** (`open_long` / `open_short` + `_via_vault`) — after computing
  `desired_borrow`, add `require!(debt_value ≤ max_debt_for_depth(S))`.
- **Liquidation handlers** — swap the flat `treasury.liquidation_bonus_bps` /
  `LIQ_FULL_BONUS_LTV_BPS` inputs for the derived pair. No other change.
- **Depth source** — gate Rails 1 & 2 on a **manipulation-resistant depth**
  (baseline reserves or a min-over-window), **not** instantaneous `pool_sol`.
  Otherwise an attacker flash-adds LP, unlocks higher LTV / a bigger position,
  opens, and pulls the LP. Liquidation Rail 3 already reads live reserves (correct —
  it wants the real unwind cost at the moment of liquidation).
- **SDK** — `getDepthMaxLtvBps` mirrors the same formula; add a `maxPositionDebt`
  helper so the UI can show the size cap; bonus display derives from `ρ_max`.

## Decisions

1. **Continuous concave LTV, not steps.** Removes cliff edges; one formula; the
   `α` dial replaces four hand-picked tier values.
2. **Anchor on `S_floor = 100 SOL`** (the smallest pool we offer post-migration),
   `LTV_min = 30%` there. The curve is *defined by the product floor*, not arbitrary
   breakpoints.
3. **Add a depth-linear size cap (`ρ_max·S`).** This is the genuinely missing rail
   and the thing that makes the bonus tractable.
4. **Flat bonus derived from `ρ_max`, with `full_bonus_ltv` coupled to the
   insolvency line.** No `bonus(S)` curve — depth lives in the size cap.
5. **Threshold (65%) stays flat and contractual.** Depth never moves the borrower's
   liquidation point (see [risk.md] / the OTC-liquidation reasoning: liquidation is
   OTC, so at the trigger the pool is the *oracle*, not the *venue* — depth belongs
   in entry sizing and exit cost, never in the trigger).

## Parameters to finalize

| Param | Reference | Notes |
|---|---|---|
| `S_floor` | 100 SOL | product floor; anchors LTV_min |
| `LTV_min` / `LTV_max` | 30% / 60% | bounds of the slide |
| `α` | 1 | conservatism dial; ≤1 to stay low through mid pools |
| `ρ_max` | 25% | size cap; sets the whole bonus chain |
| bonus safety | 1.3× | liquidator margin over slippage |

These want a sweep against real liquidator economics (gas, expected hold time,
adverse drift during the unwind) on the thinnest pool we'd offer — not just the
first-order slippage model here.

## Tripwires (when to revisit)

- Bad debt appears on pools near `S_floor` → `ρ_max` too high or bonus safety too
  thin; tighten `ρ_max` first (it cascades into the bonus automatically).
- Liquidations consistently fire near `full_bonus_ltv` (system running hot) →
  steepen the ramp (raise the bonus floor at the threshold) so positions clear
  earlier.
- Opens frequently blocked by Rail 2 on healthy pools → `ρ_max` too low, or the
  manipulation-resistant depth measure is lagging real depth; loosen `ρ_max` or
  shorten the depth window.
- LP removal observed stranding live leverage → the depth a borrower was
  underwritten against collapsed; address on the **LP** side (removal that respects
  outstanding leverage), never by re-gating the borrower.

## Deferred (worth doing later, not now)

- **Non-unit `α` on-chain.** Ship `α=1` (division-only, Kani-clean) first; add a
  fixed-point pow / lookup only if the sweep says the curve needs a different knee.
- **Dynamic (per-position live-ρ) bonus** instead of flat-`ρ_max`. Strictly more
  precise (prices each position's actual unwind), but unnecessary once Rail 2 caps
  ρ — and a flat bonus is far easier to reason about and verify. Revisit only if we
  ever drop the size cap.
