# V21 — Per-Token Closed Leveraged Markets (Design)

## Goal

Make each torch token a self-contained leveraged market. Every directional position (long or short) is a **protocol-custodied unit** that opens atomically, closes atomically via the same DeepPool, and never leaks capital outside that token's economic system until the user explicitly exits.

V21 replaces two V20 patterns: (1) generic-margin loans (open-loop SOL borrowing), and (2) user-custodied shorts (borrowed tokens placed directly in the user's wallet, where they look identical to spot holdings). Both collapse into a single coherent layer: **symmetric atomic open + custodied positions + per-token economic closure**.

## Why now

V20 had two structural issues:

1. **Margin loans were open-loop.** Borrowed SOL could leave to any Solana program. Treasury capital escaped the token's market.
2. **Shorts placed borrowed tokens in the user's wallet.** This created a category collapse in the wallet view — leveraged tokens were indistinguishable from spot tokens, breaking portfolio trackers, tax tools, and user intuition. The position's "shortness" depended on the user manually selling, and proceeds (if sold) could exit the token's market.

V21 collapses both: leverage becomes protocol-custodied (no wallet confusion, no leaks). The closed-loop property becomes structural, not opt-in.

## The layered architecture

Torch composes three layers, each with one concern:

```
Layer 0:  DeepPool AMM           immutable, generic CPMM, Jupiter-routable, no leverage knowledge
Layer 1:  Torch token             bonding, migration, fee splits, treasury, harvest
Layer 2:  Torch leverage          positions, custodied vaults, atomic open/close — all routes through Layer 0
```

Layer 0 doesn't know Layer 2 exists. The token (Layer 1) can still be spot-traded on Jupiter — the leverage layer is purely additive. Layer 2's invariants compose on top of Layer 0's swap correctness, but Layer 0 is unchanged.

**Per-token closure means:** each torch token's spot market (Layer 0) and leveraged market (Layer 2) form one closed economic system, and capital that enters that system stays in it until the user explicitly exits via close.

## The closed-loop principle (sharpened)

> A protocol primitive is **per-token closed-loop** if every leg of every operation — both borrowed asset and converted asset — stays inside that specific token's economic system until the user explicitly exits.

| Primitive | Capital flow | Per-token closed? |
|---|---|---|
| Buy (spot) | wallet SOL → pool; pool tokens → wallet | ✅ (spot, no debt) |
| Sell (spot) | wallet tokens → pool; pool SOL → wallet | ✅ (spot, no debt) |
| V20 margin | treasury SOL → wallet → anywhere on Solana | ❌ (removed) |
| V20 short | lock tokens → wallet (user may sell anywhere or keep) | ⚠️ (self-constrained, leaky on bought-side) |
| **V21 short** | SOL in → atomic pool_sell → position SOL vault holds proceeds; close pool-routed | ✅ |
| **V21 long** | tokens in → atomic pool_buy → position token vault holds proceeds; close pool-routed | ✅ |

Until close, no leveraged capital reaches a wallet. At close, surplus (PnL) routes to the user's wallet via the same pool. Total fee revenue per position lifecycle is maximized — every leg routes through Layer 0.

## Symmetric atomic, fully custodied

Both primitives are structural mirrors. Both atomic on open, both custodied for the duration, both pool-routed on close.

| | V21 Short | V21 Long |
|---|---|---|
| Collateral asset | SOL | Tokens |
| Borrowed asset | Tokens (from lock vault) | SOL (from treasury) |
| Atomic action on open | Borrow tokens → pool_sell → SOL lands in position SOL vault | Borrow SOL → pool_buy → tokens land in position token vault |
| Position vault holds | Collateral SOL + sale-proceeds SOL (single bucket) | Collateral tokens + bought tokens (single bucket) |
| User's wallet after open | Unchanged (minus collateral, minus tx fees) | Unchanged (minus collateral, minus tx fees) |
| Debt | Tokens | SOL |
| Bet direction | Price drops → token debt cheaper → close yields surplus SOL | Price rises → vault tokens worth more → close yields surplus SOL after repaying debt |
| Close (atomic) | Vault SOL → pool_buy → repay token debt → surplus SOL to user | Vault tokens → pool_sell → repay SOL debt → surplus SOL to user |
| Liquidation | Liquidator pays tokens → seizes vault SOL + bonus | Liquidator pays SOL → seizes vault tokens + bonus |

**What the user holds while a position is open:** only the right to act on it (close, partial-close). They cannot accidentally spend, transfer, or use the leveraged assets — those live in the position vault. Wallet view stays clean: spot holdings only.

---

## Decisions

### D-1: Both primitives are protocol-custodied, atomic, symmetric

V20's generic margin is removed. V20's user-custodied short is reshaped into V21's atomic custodied short. V21's leveraged long is the structural mirror. Both share the same lifecycle shape, account layout, math primitives, and liquidation logic.

### D-2: Per-position vaults

Each open position creates one vault for its held asset:

- **Short position**: `position_sol_vault` PDA, seed `[SHORT_VAULT_SEED, user, mint, position_index]`, holds SOL (collateral + sale proceeds).
- **Long position**: `position_token_vault` ATA owned by torch_market, address derived per-position, holds tokens (collateral + bought).

`position_index: u32` allows multiple positions per (user, mint, side) — supports DCA / scaled entries / strategy composition within a single token.

**Per-position vs per-token shared vault:** shared vaults require tracking each user's claim on the shared balance, which is what positions already encode. Per-position vaults make the math unfalsifiable — `vault.amount` *is* the position's held asset by definition. Derived state over tracked state (see [derived state](../../../memory)).

### D-3: Position account (single shape, side discriminant)

```rust
#[account]
pub struct Position {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub side: PositionSide,           // Long or Short
    pub position_index: u32,
    pub collateral_amount: u64,        // initial deposit (record-keeping only)
    pub debt_amount: u64,              // owed asset — tokens for short, SOL for long
    pub accrued_interest: u64,         // in debt-asset denom
    pub last_slot: u64,
    pub bump: u8,
    pub vault_bump: u8,
}
```

`held_amount` is **not** a field — it's `position_vault.amount`. Read from the vault, not from tracked state.

### D-4: Atomic open (single ix per side)

Both opens charge a flat `OPEN_FEE_BPS = 50` (0.5%) on the **borrow value in SOL**, routed to treasury. The fee is leverage-proportional — a 50% LTV position pays half what a 100% LTV one pays — so conservative borrowers don't subsidize aggressive ones. See D-9 for the economic rationale.

```
open_short(collateral_sol, min_sol_out_from_sale):
  1. Validate: pool depth band, lock has tokens, per-user borrow cap
  2. Compute borrow plan at pre-swap price + LTV bound:
       desired_borrow_value_sol = collateral_sol * effective_ltv_bps / 10_000
       open_fee_sol = desired_borrow_value_sol * OPEN_FEE_BPS / 10_000
       net_collateral_sol = collateral_sol - open_fee_sol
       tokens_to_borrow = (net_collateral_sol * effective_ltv_bps * pool_tokens)
                          / (pool_sol * 10_000)
  3. Transfer open_fee_sol: user → treasury (lamport shift, no fee)
  4. Transfer net_collateral_sol: user → position_sol_vault
  5. CPI deep_pool::swap(direction=sell, amount_in=tokens_to_borrow, min_out=min_sol_out_from_sale)
     source = lock_vault (signed by treasury_lock PDA), destination = position_sol_vault
  6. Persist Position { side: Short, collateral_amount: net_collateral_sol,
                        debt_amount: tokens_to_borrow, ... }
  7. Increment treasury.total_tokens_lent
  8. emit_cpi! OpenShortEvent { open_fee_sol, ... }

open_long(collateral_tokens, min_tokens_out_from_buy):
  1. Validate: pool depth band, treasury floor, per-user borrow cap
  2. Transfer collateral_tokens: user → position_token_vault
  3. Compute borrow plan at pre-swap price + LTV bound:
       collateral_value_sol = collateral_tokens * pool_sol / pool_tokens
       desired_borrow_sol = collateral_value_sol * effective_ltv_bps / 10_000
       open_fee_sol = desired_borrow_sol * OPEN_FEE_BPS / 10_000
       atomic_buy_sol = desired_borrow_sol - open_fee_sol
  4. Decrement treasury.sol_balance by desired_borrow_sol
     (treasury releases the full borrow, then immediately receives open_fee_sol back below)
  5. Increment treasury.sol_balance by open_fee_sol (effectively: only atomic_buy_sol leaves)
  6. CPI deep_pool::swap(direction=buy, amount_in=atomic_buy_sol, min_out=min_tokens_out_from_buy)
     source = treasury (signed by treasury PDA), destination = position_token_vault
  7. Persist Position { side: Long, collateral_amount: collateral_tokens,
                        debt_amount: desired_borrow_sol, ... }
     (debt is full desired_borrow — user owes back what was lent on their behalf,
      including the fee portion that went to treasury)
  8. Increment treasury.total_sol_lent_to_longs by desired_borrow_sol
  9. emit_cpi! OpenLongEvent { open_fee_sol, ... }
```

**Fee economics:**
- Fee is SOL-denominated regardless of position side → treasury always grows in SOL, simplifying buyback/lending math.
- Fee is taken on the SOL leg (collateral for shorts, borrow for longs) → no token-side accounting needed.
- For longs, debt records the full pre-fee borrow because the user benefits from the full SOL leaving treasury (0.5% becomes treasury revenue, 99.5% becomes their leveraged exposure). They owe back what was lent on their behalf, which is the gross.
- For shorts, debt records `tokens_to_borrow` as before — the fee reduces effective collateral but doesn't compound into the token debt.

**Slippage protection** via `min_*_out`. If swap fails, whole ix reverts — collateral and fee return to user, no half-open state. Solana tx atomicity does the work; no need for compensation logic.

### D-5: Atomic close (single ix per side)

Both close handlers route through DeepPool, settle debt, and surface surplus to user in **SOL**. The asymmetry between them (compute-exact vs sell-all) reflects the vault asset type, not a UX difference — the user always exits in SOL.

```
close_short(min_surplus_sol_out):
  1. Accrue interest (token-denominated)
  2. total_token_debt = position.debt_amount + position.accrued_interest
  3. debt_gross = math::gross_up_for_transfer_fee(total_token_debt)
     (lock_vault must receive net == total_token_debt; gross-up covers the
      Token-2022 fee on the deep_pool → lock_vault transfer leg — see V20
      invariant in math.rs:116, short.rs:242-253)
  4. sol_needed = deep_pool::quote_buy_in_for_out(debt_gross)
  5. Require position_sol_vault.amount >= sol_needed (else: partial close)
  6. CPI deep_pool::swap(direction=buy, amount_in=sol_needed, min_out=debt_gross)
     source = position_sol_vault, destination = lock_vault (repays borrow at gross)
  7. surplus_sol = position_sol_vault.amount   (whatever's left after the buy)
  8. Require surplus_sol >= min_surplus_sol_out (slippage protection)
  9. Transfer surplus_sol: position_sol_vault → user (lamport shift, no fee)
  10. Decrement treasury.total_tokens_lent by total_token_debt; increment treasury.short_interest_collected
  11. Close position + vault accounts, reclaim rent to user
  12. emit_cpi! CloseShortEvent

close_long(min_surplus_sol_out):
  1. Accrue interest (SOL-denominated)
  2. total_sol_debt = position.debt_amount + position.accrued_interest
  3. CPI deep_pool::swap(direction=sell, amount_in=position_token_vault.amount, min_out=total_sol_debt)
     source = position_token_vault (sells ALL vault tokens in one swap)
  4. sol_out = total SOL returned by swap (after pool fee + token transfer fee on input leg)
  5. Require sol_out >= total_sol_debt (else: partial close or trigger liquidation path)
  6. Transfer total_sol_debt: → treasury (repays borrow, lamport shift, no fee)
  7. surplus_sol = sol_out - total_sol_debt
  8. Require surplus_sol >= min_surplus_sol_out
  9. Transfer surplus_sol: → user (lamport shift, no fee)
  10. Decrement treasury.total_sol_lent_to_longs; increment treasury.long_interest_collected
  11. Close position + vault accounts, reclaim rent to user
  12. emit_cpi! CloseLongEvent
```

**Asymmetric routing, symmetric exit:**
- `close_short` computes exact SOL needed to repay debt (with gross-up for the token transfer fee on the lock-bound leg), spends that, hands the leftover SOL to user.
- `close_long` sells all vault tokens in one swap, splits the SOL output (debt → treasury, surplus → user).
- Both exits land entirely in SOL in the user's wallet. No "what to do with surplus tokens" question, no wallet category collapse.

**Gross-up only where needed.** Token-debt repayment (short close, short liquidation) requires `gross_up_for_transfer_fee` because Token-2022 fees apply on every token transfer leg. SOL-debt repayment (long close) needs no gross-up — SOL transfers are lamport shifts with no fee. This matches V20's invariant: lock_vault is perfectly conserved by making the borrower implicitly absorb the open-leg fee and explicitly pay the close-leg fee, all of which accumulates as protocol revenue via the harvest cycle.

**No wallet-repay variant.** There's no user-side asset to repay with — everything's in the vault. Close is always pool-routed. Simpler than V20's dual close paths.

**Partial close** is possible: user specifies a `repay_fraction_bps` (default 10000 = full close) and the handler scales the swap accordingly. Position stays open with reduced debt + vault balance. Same gross-up logic applies on the partial amount.

### D-6: Liquidation (symmetric)

```
liquidate_short(liquidator, position):
  1. Accrue interest, compute LTV
  2. require!(ltv > DEFAULT_LIQ_THRESHOLD)
  3. debt_to_cover = total_debt * DEFAULT_LIQ_CLOSE_BPS / 10_000     [tokens]
  4. sol_seize = debt_to_cover * (10_000 + DEFAULT_LIQ_BONUS_BPS) / 10_000 * pool_sol / pool_tokens
  5. Liquidator → lock_vault: debt_to_cover tokens (repaying borrow)
  6. position_sol_vault → liquidator: min(sol_seize, vault.amount) SOL
  7. Update position.debt_amount -= debt_to_cover
  8. If vault drained but debt remains: bad debt write-off (treasury absorbs via total_tokens_lent decrement)
  9. If position fully closed: return residual vault.amount to borrower, close accounts

liquidate_long: mirror — liquidator pays SOL → treasury, seizes tokens + bonus from position_token_vault.
```

Bad debt accounting: position vault is the entire backstop for the position. If vault can't cover the seize amount at threshold breach, vault drains and remaining debt is written off against the corresponding aggregate (`total_tokens_lent` for shorts, `total_sol_lent_to_longs` for longs). This is the same shape as V20 short liquidation, just sourcing from per-position vaults instead of treasury-pooled collateral.

### D-7: Interest accrual

Formula unchanged from V20:

```
interest_delta = debt_amount * DEFAULT_INTEREST_RATE_BPS * slots_elapsed / (10_000 * EPOCH_DURATION_SLOTS)
```

**Rate change:** `DEFAULT_INTEREST_RATE_BPS`: 200 → **150** (2% → 1.5% APR).

Token-denominated for shorts (paid by including interest in the gross-up on close), SOL-denominated for longs (paid out of close-side SOL surplus before user payout). Interest revenue accumulates in treasury counters (`short_interest_collected`, `long_interest_collected`).

The interest rate drop pairs with the new open fee (D-9) to keep total cost-of-leverage roughly constant for medium-term holders while shifting revenue forward in time.

### D-8: Treasury role

Treasury narrows to four hats, none of which custody open-position assets:

- **Short backstop**: `lock_vault` holds bonded supply available for shorting.
- **Long backstop**: `treasury.sol_balance` funds SOL borrows for long opens.
- **Fee collector**: bonding splits, harvest, position interest, liquidation revenue.
- **Buyback engine**: `swap_fees_to_sol` cycles accrued fee tokens back to SOL.

`total_tokens_lent` and `total_sol_lent_to_longs` track aggregate exposure — required for the `MIN_TREASURY_SOL_FOR_LENDING` gate and global utilization caps. These are tracked (not derived) because iterating all positions on every open is too expensive on-chain.

**Removed from V20:** `short_collateral_sol` (collateral SOL now lives in per-position vaults, not pooled in treasury).

### D-9: Cost-of-leverage structure

V21 reshapes the fee structure to grow the treasury (and therefore the lending pool) faster while keeping medium-term costs roughly neutral:

| Component | V20 | V21 |
|---|---|---|
| Open fee | 0 | **0.5% of borrow value in SOL** (`OPEN_FEE_BPS = 50`) |
| Interest rate | 2% APR (`200 bps`) | **1.5% APR (`150 bps`)** |

**Cost over hold duration:**

| Hold | V20 (2% APR) | V21 (0.5% open + 1.5% APR) | Notes |
|---|---|---|---|
| 1 day | 0.005% | 0.504% | scalper barrier |
| 1 week | 0.04% | 0.53% | |
| 1 month | 0.17% | 0.625% | |
| 3 months | 0.5% | 0.875% | |
| **1 year** | **2.0%** | **2.0%** | **break-even point** |
| 2 years | 4.0% | 3.5% | V21 cheaper |

**Design intent — natural user selection:**

- Scalpers / dust positions: penalized (0.5% upfront vs ~0.005% on V20 for sub-day holds). Filters out noise traders who don't generate meaningful pool fees but consume protocol resources.
- Medium-term holders (weeks–months): pay ~0.5–1% more than V20. Small premium for the per-token closed-market structural guarantees.
- Long-term holders (1y+): break-even or cheaper than V20. Rewarded for providing market stability.

**Treasury growth path:**

- Open fees are SOL, deposited directly to treasury, immediately available for long lending (or buybacks).
- Interest still SOL or token, follows existing harvest cycle.
- Combined: treasury growth rate increases proportional to leverage volume, accelerating the lending pool unlock under `MIN_TREASURY_SOL_FOR_LENDING`.

**Open fee scaling property:**

Fee is computed against **borrow value in SOL**, not collateral. A 50% LTV position pays half what a 100% LTV one pays. Conservative borrowers don't subsidize aggressive ones, and the fee scales with the risk/lending capacity the position consumes.

### D-10: TWAP-hardened liquidation mark (oracle-manipulation defense)

**Problem (sim-confirmed).** V21 marks position LTV — both the liquidation
*trigger* and the *seize* sizing — at the raw spot ratio (`pool_sol / pool_tokens`;
see `calc_collateral_value` / `calc_short_debt_value`). DeepPool is a generic CPMM
with no oracle, so that mark is an unguarded spot price. Sim scenario
`oracle_liquidation_attack` (+ depth×size sweep) shows an attacker with capital
and *no stake in the victim* can pump the pool to push a **healthy** position —
one opened at the protocol's own depth-band max LTV — past the 65% threshold,
liquidate it, and over-seize the collateral at the inflated mark. Net-positive
EV across every pool depth (10→700 SOL) and victim size (≈16–25% of the victim's
collateral), conservation-verified. Atomic in one transaction (`pool_buy` CPI →
`liquidate`), so the victim cannot react. The depth-band cap guards the *open*;
nothing guarded the *mark*. This is the canonical spot-AMM-as-oracle class
(Mango-style).

**Decision.** Mark the liquidation **trigger** and **seize accounting** against a
**hardened TWAP** of the spot pool, never raw spot. Lift the TWAP primitive
already designed and verified in `torch_perp` (whose funding index faces the same
oracle-free-spot surface): a cumulative-reserve observation ring (`cumulative_sol`,
`cumulative_token` advanced by `reserve × slot_delta`), read oldest→newest as
`twap = Σsol_delta / Σtoken_delta`, behind three structural gates —

| Gate | Constant | Property |
|---|---|---|
| Observation spacing | `MIN_OBSERVATION_SPACING_SLOTS = 500` | rate-limits ring compression; floors the effective TWAP window (`ring × spacing`) regardless of crank frequency |
| Liquidity gate | `MIN_SPOT_POOL_SOL = 5 SOL` | thin pools (cheap to move) don't feed the mark; reuses the lending floor — one "too thin to trust" boundary |
| Per-obs clamp | `MAX_PER_OBS_DEVIATION_BPS = 100` (1%) | if spot deviates >1% from the prior step's implied price, freeze to last-good reserves until spot returns to band; non-lossy within band |

A single-tx pump records no observation (no slot advance) so it can't move the
mark → the trigger (LTV-at-TWAP) stays below threshold → the manufactured
liquidation is refused. (torch_perp sim: a 438× spot crash held the TWAP within
0.0001% of reference; the clamp/cap math carries 11 Kani harnesses — reuse them.)

**Clamp variant — ratchet, not freeze (sim finding).** torch_perp's clamp
*freezes* the index at last-good until spot returns to band; correct for funding
(a stale index only drives bounded, capped accrual). The sim
(`oracle_mitigation_test`) shows freeze is **wrong for a liquidation mark**: under
a legitimate sustained move it never lifts the trigger, so a genuinely underwater
position can never be liquidated → unbounded bad debt (no funding-cap backstop
here). The liquidation mark therefore **ratchets** — each out-of-band observation
records a price clamped ≤1% toward spot — so it eventually tracks a real move
while a one-tx pump still can't move it. Empirically: the manufacture-from-open
attack flips from **+9.6 SOL** to **−0.27 SOL (blocked)** with the guard on, and a
real 93% move still liquidates after a bounded lag (~86 observations at 1%/obs ×
500-slot spacing). The lag is the cost; `MAX_PER_OBS_DEVIATION_BPS` / spacing /
ring size tune manipulation-resistance vs tracking speed, and per-position
custody (D-2) bounds the bad debt that can accrue during the lag.

**Why not a vAMM (torch_perp's mark) here.** torch_perp marks on a *synthetic*
vAMM and never swaps the spot pool on settlement — mark and payout are both
internal, so spot only feeds funding. V21 is the opposite: it settles
**physically** (open/close/liquidate are real pool swaps; the vault holds real
SOL/tokens). A synthetic mark on a physically-settled position creates basis
risk — you'd mark "healthy" while the real close realizes a loss, and the gap is
itself farmable. And a V21 vAMM would have *no synthetic order flow* to price it
(all flow routes through the real pool), so it degenerates into a TWAP with extra
steps. The vAMM belongs to the synthetic product (torch_perp); V21 keeps its
physical/closed-loop identity and hardens the *real-pool* mark instead.

**Residual (inherent to physical settlement).** The TWAP gates the trigger and
bounds the seize *accounting*, killing manufactured liquidations. But the actual
seize *execution* is still a real swap at real spot — V21 cannot escape the real
pool the way a synthetic perp does. The clamp shrinks the exploitable window to
"can't fake the trigger, can't over-credit the seize": a determined attacker must
now hold an off-market price across the whole TWAP window (expensive, exposed to
arb and to the victim closing), not snipe it atomically.

**Trade-off.** TWAP-marking lags a genuine fast gap → liquidation is delayed until
the TWAP catches up, re-opening the D-6 gap-bad-debt window. Acceptable because
per-position custody already **bounds** that loss to the single position's debt
(sim `long_baddebt_severity`: bad debt is 0 up to ~55% crash and stays capped at
the position's own debt even in a 98% wipeout). Manipulation-resistance is bought
with bounded, already-contained lag risk.

**Parameters — the liquidation mark needs its OWN, not funding's** (sim
`twap_tuning`, ring × band sweep). **Band dominates lag** (≈ move/band
observations); ring only trims the averaging tail (~ring/2 obs). The decisive
finding: the lag to liquidate a real move **equals** the time an attacker must
HOLD an off-market price to manufacture a trigger — *one knob, not two*. A
symmetric ratchet TWAP cannot make liquidation both fast and force a long
manipulation-hold. What it does: convert an **atomic** theft (the #1 finding —
blocked at every param, EV −0.27 SOL) into a **timed, capital-intensive hold**
whose real cost is arb-bleed + fees + locked capital across the lag — a cost a
genuine move doesn't pay. Consequences:

- **Separate params from funding.** Funding wants a long, smooth window (ring 32,
  1%/obs). Liquidation wants responsiveness: **ring 8–16, band ~300 bps (3%/obs),
  spacing 500** → ~90–130 min lag (= forced manipulation-hold). They share the
  primitive, not the constants.
- **The TWAP is not sufficient alone** against a sustained-hold manipulator. Pair
  it with: (a) per-position custody — already bounds the loss accrued during the
  lag (D-2); (b) a **bonus/seize cap** so even a manufactured liquidation extracts
  too little to repay the hold cost; (c) arb, which raises the hold cost (useful,
  not load-bearing). The TWAP's role is to kill the atomic attack and force any
  manipulation into a slow, observable, expensive hold the other layers then make
  uneconomic.

**Seize-accounting clamp (implemented + validated).** Beyond the trigger, the
liquidation *seize* is priced at the same hardened mark, not raw spot — sim
`twap_tuning` shows the SOL seized is invariant (0.00% drift) to a 3.2× spot pump
at liquidation. So neither the trigger nor the seize can be manipulated atomically.

**Distress-scaled bonus cap (implemented + validated).** The liquidation bonus
ramps 0 → full from `DEFAULT_LIQ_THRESHOLD` to `LIQ_FULL_BONUS_LTV_BPS` (90%),
measured on the hardened LTV. Sim `liq_bonus_ramp`: a just-over-threshold (66%)
liquidation earns **0.24%** bonus; a genuine deep one (93%) earns the full **10%**.
This removes the *prize* from the residual sustained-hold manipulation — an
attacker can only push a position just over the line, where the bonus is ~0 —
completing the stack: **trigger guard + seize clamp + bonus cap + per-position
custody.**

---

## What V21 removes

- V20 generic margin handlers (`borrow`, `repay`, `liquidate_long`) and contexts
- V20 via-vault margin variants
- `LoanPosition` account type
- `total_sol_lent` field on `Treasury`
- `short_collateral_sol` field on `Treasury` (now per-position)
- V20 short's "tokens to wallet" open semantics
- V20 short's "user supplies tokens from wallet" close variant
- SDK builders for V20 margin and V20 short flows

## What V21 preserves

- DeepPool: **zero changes**. Generic CPMM. Jupiter listing thesis intact.
- Bonding curve, migration, treasury fee splits, harvest, buyback gating: unchanged.
- Token-2022 handling, transfer fees, extension blocklist: unchanged.
- Kani proofs for AMM math: unchanged.
- Per-user borrow share cap (`MAX_USER_BORROW_SHARE_BPS`), depth-band LTV caps, liquidation parameters (`DEFAULT_LIQUIDATION_*`): unchanged.

## What V21 reshapes from V20

V20 shorts are **not preserved as-is**. The semantic shape changes:

| | V20 short | V21 short |
|---|---|---|
| Open output | Tokens to user wallet | SOL to position vault (atomic sell) |
| User must do | Sell tokens to be "actually short" | Nothing — open IS the short |
| Close input | User supplies tokens from wallet | Vault SOL pool-buys tokens to repay |
| Collateral location | Treasury (pooled `short_collateral_sol`) | Per-position SOL vault |

---

## Open questions

1. **Position discovery on close.** User has multiple positions per (mint, side) due to `position_index`. Close ix takes the position PDA address directly; SDK helper lists positions for a user. Frontend resolves "close my oldest long" → specific PDA.
2. **Partial-close semantics.** Single `repay_fraction_bps` arg vs explicit `amount_to_close`. Lean fraction (always relative to current debt), but worth confirming.
3. **`MIN_TREASURY_SOL_FOR_LENDING` recomputation.** V21: `treasury.sol_balance - total_sol_lent_to_longs >= MIN`. Short collateral no longer in treasury, simpler check than V20.
4. **Compounding via re-collateralization.** User opens long, vault tokens grow on price rise — can they use those (still in vault) as collateral for a second long? **No** — vault tokens are committed. They'd need to close (surface SOL) and use fresh collateral. Per-user cap still applies.

## Future work (V22+)

### Cross-position netting on liquidation

When a user holds both a long and a short on the same mint, atomic-close the opposing position first to recover the threatened side. **Much easier with custody** — all assets are in known vaults, no wallet coordination needed. Likely a single handler that pre-routes proceeds.

### Transferable positions

A position is a clean on-chain object (PDA + per-position vault). `transfer_position(new_owner)` ix hands it off — enables OTC markets in unrealized PnL, position-based payments, and structured products.

### Position-as-collateral

A leveraged long position with positive PnL could collateralize another position. Recursive composition stays closed-loop because it's all per-token.

### Position NFTs

Wrap each position PDA as a metaplex NFT, surface in wallet UIs, render PnL in image. Pure UX layer, no protocol change.

---

## Implementation plan

1. **Sim first** (`sim/torch_sim.py`):
   - Replace `open_short` with atomic-custodied variant (atomic pool_sell, output to position vault)
   - Replace `open_leveraged_long` user-wallet variant with custodied variant
   - Per-position vault dict on TorchSim instance (mirror of on-chain PDA per position)
   - Replace single `close_*` paths with atomic pool-routed close
   - Update all scenarios — they shouldn't read from user wallet for position state
   - Add `scenario_per_token_closure` invariant scenario

2. **Math** (`programs/torch_market/src/math.rs`):
   - `calc_position_ltv`, `calc_open_borrow_size`, `calc_close_pool_amount_in`, `calc_liquidation_seize`
   - Kani proofs: overflow safety, LTV monotonicity, liquidation bound, vault sufficiency, conservation

3. **State + contexts** (`state.rs`, `contexts.rs`):
   - Unified `Position` account with `PositionSide` enum
   - `position_sol_vault` PDA + `position_token_vault` ATA contexts
   - Open/Close/Liquidate × {Short, Long} × {direct, via_vault} contexts

4. **Handlers** (`handlers/leverage.rs`):
   - `open_short`, `open_long`, `close_short`, `close_long`, `liquidate_short`, `liquidate_long`
   - All `*_via_vault` variants for Torch Vault integration

5. **Tests**:
   - Litesvm e2e for all six handlers × happy + edge paths
   - Property test: closed-loop conservation (no leveraged asset reaches a wallet between open and close)
   - Loadtest: 1k open/close cycles, measure CU + fee accrual

6. **SDK** (`packages/sdk`):
   - Builders for all six handlers
   - Position discovery helpers (list user positions by mint)
   - Update IDL artifacts

7. **Remove V20 margin + V20 short handlers** (per "What V21 removes")

8. **Deployment**: V21 is the first deploy. No state migration.

---

## D-12: via_vault leverage (TorchVault custody integration)

Parallel `*_via_vault` variants of the six leverage handlers so a `TorchVault`
(shared SOL-custody account) can run leverage through its **linked wallets**,
mirroring the existing `buy_via_vault` / `sell_via_vault` spot pattern.

### Security model (load-bearing — the whole point)

Two roles, two powers, already enforced by the spot vault handlers:
- **Authority** (`torch_vault.authority`, `has_one = authority`): the ONLY role
  that moves funds OUT of the vault — `withdraw_vault`, `link_wallet`,
  `transfer_authority`. Untouched here.
- **Linked wallet** (`vault_wallet_link` PDA seeded on the signer, constrained
  `vault_wallet_link.vault == torch_vault`): may EXECUTE trades with vault funds
  but can NEVER extract value to a wallet.

**Invariant:** a linked wallet moves value only **vault → position → vault**,
never position→wallet or vault→wallet. The worst a compromised linked wallet can
do is make bad trades; it cannot drain the vault. Only the authority extracts.

### Ownership + flows

- **Positions are vault-seeded:** `[POSITION_SEED, torch_vault, mint, side, idx]`
  (and the per-position vaults seed off `torch_vault`, not a wallet). The vault is
  the economic owner; any linked wallet can open/close/manage; this is what lets
  custody outlive any single wallet.
- **Collateral comes from the vault**, all surplus/residual on close returns to
  the vault (real lamports + `sol_balance` + `total_received`). The signing linked
  wallet pays only its own operational rent (`payer = signer`), reclaimed to
  itself on close — its money, never vault funds.
- **Liquidation** takes an external liquidator (no `vault_wallet_link`); seized
  assets → liquidator as today; the position is vault-seeded so the residual → vault.

### Per-handler delta vs the wallet-funded sibling (logic is otherwise identical)

| Handler | Auth | Collateral source | Token sink / surplus → |
|---------|------|-------------------|------------------------|
| `open_short_via_vault` | linked wallet | vault SOL (`sol_balance` −, `total_spent` +) → position_sol_vault | borrowed tokens sold; proceeds in position_sol_vault |
| `close_short_via_vault` | linked wallet | position_sol_vault | surplus SOL → vault (`sol_balance`+, `total_received`+); rent → signer |
| `liquidate_short_via_vault` | external liquidator | — | seize → liquidator; residual → vault |
| `open_long_via_vault` | linked wallet | vault token ATA → position_token_vault | borrowed SOL buys; tokens in position_token_vault |
| `close_long_via_vault` | linked wallet | position_token_vault | surplus SOL → vault; rent → signer |
| `liquidate_long_via_vault` | external liquidator | — | seize tokens → liquidator; residual tokens → vault ATA |

Shared pure logic (sizing, fees, debt accounting, TWAP mark) stays in `math.rs` —
the via_vault handlers differ only in the funding edges + the position seed.

---

## Success criteria

- All non-removed torch scenarios PASS (V20 margin scenarios deleted)
- `scenario_per_token_closure`: no leveraged asset reaches a wallet between open and close, across 1500 random ops
- `scenario_basis_trade`: long + short on same mint, net PnL = fees + slippage (delta-neutral confirmation)
- `scenario_directional_profitability`: long profits on up moves, short profits on down moves, all PnL settled in SOL at close
- Kani proofs on new math: all pass
- DeepPool: zero changes, Jupiter integration story intact
- Total ix count: V20's 40 minus ~9 (margin + V20 short variants) plus ~12 (V21 six handlers + via_vault) ≈ similar or slightly higher
- Wallet view of a user with N open positions: indistinguishable from a user with no positions (modulo collateral debits)
