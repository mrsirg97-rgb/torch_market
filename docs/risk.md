# Depth-Anchored Risk Model for Permissionless On-Chain Lending

*A formal analysis of Torch Market's layered defense system*

---

## Abstract

We present a lending model for constant-product AMM pools built on three depth-scaled rails, each a pure function of pool liquidity depth evaluated once at position open. **Rail 1** sets the maximum loan-to-value ratio on a continuous, concave curve — 30% at the 100-SOL floor (the smallest pool we lever) rising toward a 60% asymptote. **Rail 2** caps a single position's debt value at 25% of pool depth, which makes the worst-case liquidation-unwind slippage *depth-invariant*. **Rail 3** derives a single flat liquidation bonus (32.5%) from that cap, so every position — on every pool — unwinds within one bonus, with no depth-varying schedule to defend. Combined with per-user borrow caps ($\mu = 23$) proportional to total token supply, an absolute 20%-of-lendable per-user ceiling, a global utilization ceiling, and a treasury-activity unlock gate keyed on the protocol's earned SOL, the system creates a graduated risk regime: pools below the floor get no leverage, thin pools lever conservatively, and deep pools graduate into functional margin markets — while the per-user cap can impose a stricter effective LTV still on a small treasury. Short positions do not share the longs' structural protection due to the asymmetric nature of upward price movement, and remain liquidatable by design. The 300M token short pool is preserved across cycles by Token-2022 gross-up accounting on every close. The liquidation mark is a keeperless TWAP computed from the pool's own swap-driven price cumulative — no external oracle, no keeper, no stored baseline. The pool itself is the sole source of truth. Economic simulation confirms the model under adversarial conditions including cascade liquidations.

---

## 1. Constant Product Pool Fundamentals

A constant-product automated market maker maintains the invariant:

$$x \cdot y = k$$

where $x$ is the SOL reserve (in lamports) and $y$ is the token reserve (in base units). The spot price of a token in SOL is:

$$P = \frac{x}{y}$$

A trade that inputs $\Delta x$ SOL returns:

$$\Delta y = \frac{y \cdot \Delta x}{x + \Delta x}$$

Post-trade reserves become $(x + \Delta x, \; y - \Delta y)$, and $k$ is preserved.

### 1.1 Manipulation Cost

To move the price by a factor $\alpha$ (e.g., $\alpha = 1.2$ for a 20% pump), an attacker must input:

$$\Delta x = x \cdot (\alpha - 1)$$

**Proof.** After buying, the new SOL reserve is $x' = x + \Delta x$. The new token reserve is $y' = k / x' = xy / (x + \Delta x)$. The new price is:

$$P' = \frac{x'}{y'} = \frac{(x + \Delta x)^2}{xy}$$

Setting $P' = \alpha \cdot P$:

$$\frac{(x + \Delta x)^2}{xy} = \alpha \cdot \frac{x}{y}$$

$$(x + \Delta x)^2 = \alpha \cdot x^2$$

$$\Delta x = x(\sqrt{\alpha} - 1)$$

For small $\alpha$, $\sqrt{\alpha} - 1 \approx (\alpha - 1)/2$, so a 20% price move costs approximately $0.1x$ SOL. For a pool with $x = 500$ SOL, this is 50 SOL.

This cost scales linearly with pool depth, making deeper pools proportionally more expensive to manipulate.

---

## 2. Depth-Scaled Risk Rails

> **[V21]** The original four-step LTV *ladder* is replaced by a continuous,
> concave **curve** (Rail 1), paired with a **size cap** (Rail 2) and a **derived
> liquidation bonus** (Rail 3). Depth is priced **once at open** by three pure
> functions; the liquidation logic itself is unchanged. Full derivation:
> [`depth-scaled-risk-rails.md`](depth-scaled-risk-rails.md).

### 2.1 Rail 1 — Continuous Max-LTV Curve

We map pool SOL reserves $x$ to a maximum LTV with a concave curve anchored at the
smallest pool we lever, $S_{\text{floor}}$:

$$L(x) = \begin{cases}
0 & \text{if } x < S_{\text{floor}} \\[4pt]
\operatorname{clamp}\!\left(L_{\max} - (L_{\max} - L_{\min})\cdot\dfrac{S_{\text{floor}}}{x},\; L_{\min},\; L_{\max}\right) & \text{if } x \geq S_{\text{floor}}
\end{cases}$$

With parameters (lamports and basis points):

| Symbol | Value | Description |
|--------|-------|-------------|
| $S_{\text{floor}}$ | 100 SOL | Smallest pool we lever; below it, no leverage |
| $L_{\min}$ | 3000 bps (30%) | Max LTV **at** the floor |
| $L_{\max}$ | 6000 bps (60%) | Asymptote as $x \to \infty$ |

The form is exact-at-the-anchor: at $x = S_{\text{floor}}$, $L = L_{\max} - (L_{\max}-L_{\min}) = L_{\min}$; as $x \to \infty$, $L \to L_{\max}$. The exponent is fixed at $\alpha = 1$, so the curve is **division-only** (no `pow`, Kani-friendly). Sampled:

| Pool SOL | 100 | 200 | 500 | 1000 | 5000 | $\infty$ |
|----------|-----|-----|-----|------|------|----------|
| Max LTV | 30% | 45% | 54% | 57% | 59.4% | 60% |

### 2.2 Properties

**Monotonicity.** $L$ is non-decreasing in $x$: deeper pools permit (weakly) more leverage.

**Concavity.** $\partial L/\partial x = (L_{\max}-L_{\min})\,S_{\text{floor}}/x^2 > 0$ and decreasing — the marginal LTV bought by each additional SOL of depth diminishes. Manipulation resistance is bought cheaply at first and asymptotes, matching the $\sqrt{\alpha}$ manipulation-cost curve of §1.1.

**Self-defense.** To lift the curve, an attacker must increase pool SOL by buying tokens. This (a) deepens the pool, raising manipulation cost, and (b) moves the tokens to the attacker, who needs them as collateral. The attacker cannot inflate $L$ without increasing their own cost of attack. Unlike the old ladder, there are no tier *cliffs* to straddle — the response is smooth at every depth.

**Graceful degradation.** If pool SOL falls, $L$ falls continuously, tightening the cap on *new* positions. Existing positions are unaffected (their LTV was valid at creation) but grow more liquidatable if their backing drops.

### 2.3 Effective LTV

The protocol stores a per-token ceiling $L_T$ (`treasury.max_ltv_bps`), seeded at creation to $L_{\max} = 6000$. The effective cap is:

$$L_{\text{eff}}(x) = \min(L(x), L_T)$$

Because $L_T$ is seeded to the curve's asymptote, it never binds below the curve in the default configuration — it exists as a hard governance override should a token need a stricter ceiling than depth alone implies.

### 2.4 Rail 2 — Size Cap

Independently of the LTV curve, a single position's SOL-denominated debt value is capped at a fixed fraction of pool depth:

$$D_{\max}(x) = \rho_{\max} \cdot x, \qquad \rho_{\max} = 2500 \text{ bps } (25\%)$$

This is the load-bearing rail for liquidation economics. The worst-case slippage to unwind a position is $\approx D/x$ (the debt as a fraction of pool depth); clamping $D \leq \rho_{\max} x$ makes that slippage **depth-invariant** — at most $\rho_{\max}$ on *every* pool, large or small. A single flat liquidation bonus can therefore clear the unwind everywhere, with no depth-varying bonus schedule to defend. The cap is applied as a **clamp** at open (the position simply opens at the capped size, shown in the UI), mirroring the per-user and lock caps of §3.

### 2.5 Rail 3 — Derived Liquidation Bonus

The liquidation bonus is a flat **ceiling** $b$, derived from $\rho_{\max}$ with a safety margin $\sigma = 1.3$:

$$b = \sigma \cdot \rho_{\max} = 1.3 \times 2500 = 3250 \text{ bps } (32.5\%)$$

Because Rail 2 caps unwind slippage at $\rho_{\max}$, a bonus of $1.3\rho_{\max}$ leaves the liquidator whole with a 30% margin. The realized bonus ramps from $0$ (at the liquidation threshold) to $b$ at the **full-bonus LTV**, derived so the liquidator is exactly whole at the point insolvency would begin:

$$L_{\text{full}} = \frac{10^4}{1 + b/10^4} = \frac{10^8}{10^4 + 3250} = 7547 \text{ bps } (75.5\%)$$

The ramp is measured on the **hardened TWAP LTV**, so a manufactured barely-over-threshold liquidation earns $\approx 0$ — there is no prize for manipulation. (TWAP mark, seize clamp, and spot veto are detailed in [`v21-closed-loop-leverage.md`](v21-closed-loop-leverage.md) §D-10.)

---

## 3. Per-User Borrow Cap

### 3.1 Definition

The per-user borrow cap is the minimum of two terms — a formula cap that scales with the user's share of total supply, and an absolute ceiling at 20% of lendable SOL:

$$B_{\max}(c) = \min\left( \frac{M \cdot c \cdot \mu}{S}, \; \frac{M \cdot \beta}{10000} \right)$$

where:
- $M$ = maximum lendable SOL (utilization cap applied to available treasury balance)
- $c$ = user's collateral in base token units
- $\mu = 23$ = borrow share multiplier (formula cap)
- $\beta = 2000$ bps = `MAX_USER_BORROW_SHARE_BPS` (absolute cap, 20% of $M$)
- $S = 10^{15}$ = total token supply (1 billion tokens at 6 decimals)

### 3.1.1 Why the absolute cap is load-bearing

The formula cap scales linearly with collateral. Without an upper clamp, a user with $c/S \geq 1/\mu \approx 4.35\%$ of total supply could borrow the entire lendable pool. The absolute cap binds whenever the formula would exceed 20% of $M$, i.e. when:

$$\frac{c}{S} > \frac{\beta / 10000}{\mu} = \frac{0.2}{23} \approx 0.870\%$$

In practice, the absolute cap dominates for any meaningful borrower (anyone holding more than ~0.87% of supply, which is well below the 2% bonding-curve wallet cap). The formula cap provides graceful protection for small holders; the absolute cap is the hard ceiling for whales. Together they guarantee at least five simultaneous concentrated borrowers can be served, and that no single borrower can starve the others.

### 3.2 Interpretation

The user's maximum borrow is proportional to their share of total token supply:

$$B_{\max}(c) = M \cdot \mu \cdot \frac{c}{S}$$

A user holding 1% of total supply ($c/S = 0.01$) can borrow at most $0.01 \times 23 \times M = 0.23M$, or 23% of lendable SOL. Four such users fill the pool.

### 3.3 Implied LTV Ceiling

The per-user cap creates an implied LTV that may be stricter than the depth curve. The user's collateral value in SOL is:

$$V(c) = c \cdot P = c \cdot \frac{x}{y}$$

Their actual LTV if borrowing the maximum is:

$$\text{LTV}_{\text{cap}} = \frac{B_{\max}(c)}{V(c)} = \frac{M \cdot c \cdot \mu}{S} \cdot \frac{y}{c \cdot x} = \frac{M \cdot \mu \cdot y}{S \cdot x}$$

Note that $c$ cancels. The cap-implied LTV depends only on the pool ratio and treasury size, not on individual position size. This prevents concentration regardless of how many tokens a single user holds.

### 3.4 Numerical Examples

Post-migration pool: $x = 200$ SOL, $y = 145 \times 10^6$ tokens ($145 \times 10^{12}$ base units).

**Fresh treasury (22 SOL).** Utilization cap 80%: $M = 17.6$ SOL.

$$\text{LTV}_{\text{cap}} = \frac{17.6 \times 23 \times 145 \times 10^{12}}{10^{15} \times 200} = 0.029 = 3.0\%$$

At 3% effective LTV, the token price would need to drop **95%** before the position reaches the 65% liquidation threshold. Structurally near-impossible.

**Moderate treasury (150 SOL).** $M = 120$ SOL.

$$\text{LTV}_{\text{cap}} = \frac{120 \times 23 \times 145 \times 10^{12}}{10^{15} \times 200} = 0.20 = 20\%$$

At 20% effective LTV, a **69% price drop** triggers liquidation. Rare but real — this is a functional margin market.

**Deep treasury (500 SOL).** $M = 400$ SOL.

$$\text{LTV}_{\text{cap}} = \frac{400 \times 23 \times 145 \times 10^{12}}{10^{15} \times 200} = 0.67$$

Per-user cap exceeds the depth curve (45% for 200 SOL pool). The depth curve becomes the binding constraint at 45% LTV — a **31% price drop** triggers liquidation.

### 3.5 Treasury Graduation

The protocol naturally graduates from "near-impossible to liquidate" to "real margin market" as treasury grows:

| Treasury | Max Lendable | Effective LTV | Drop to Liquidate | Regime |
|----------|-------------|---------------|-------------------|--------|
| 22 SOL | 17.6 SOL | 3% | 95% | Protected — fresh token |
| 150 SOL | 120 SOL | 20% | 69% | Active — real margin |
| 300 SOL | 240 SOL | 41% | 37% | Mature — liquidation likely in crashes |
| 500+ SOL | 400+ SOL | 45% (depth capped) | 31% | Deep — depth curve is the ceiling |

This graduation is emergent, not designed. It arises from the interaction of three independent caps, each with a different scaling relationship to treasury size.

---

## 4. Global Utilization Cap and Activity Gate

### 4.1 Available SOL

Define the protocol's **available SOL** as gross treasury minus short collateral parked in escrow:

$$T_{\text{avail}} = T_{\text{sol}} - T_{\text{short}}$$

Short collateral is the shorter's own SOL, held to backstop the token debt. It is not protocol-earned float and must not be lent out or counted toward activity gating.

### 4.2 Utilization Cap

Total SOL lent across all positions is bounded by:

$$\sum_i b_i \leq \frac{T_{\text{avail}} \cdot U}{10000}$$

where $U = 8000$ bps (80%) and $b_i$ is user $i$'s borrowed amount.

### 4.2.1 Treasury Solvency

This guarantees $T_{\text{avail}} - \sum b_i \geq 0.2 \cdot T_{\text{avail}}$. Even if every borrower defaults simultaneously, 20% of available SOL remains unlent. Combined with seized collateral from liquidations, the treasury maintains positive balance under total default.

### 4.3 Activity Unlock Gate

Long borrows are additionally gated by a minimum available-SOL threshold:

$$T_{\text{avail}} \geq G$$

where $G$ is set per build feature:

| Build | $G$ |
|---|---|
| `simnet` (local tests) | 0 |
| `devnet` | 1 SOL |
| (default — mainnet) | 100 SOL |

The gate proves the protocol has accumulated meaningful organic float before opening the lending side. It grows from: (a) bonding-curve treasury splits, (b) bond-completion buy fees, (c) post-migration transfer-fee harvest swapped to SOL, (d) interest paid on closed long positions, (e) ratio-gated buyback proceeds. Crucially, it does NOT grow from short collateral, which is escrowed user funds.

The short side has no equivalent gate: shorts borrow tokens from the static 300M `TreasuryLock` (independent of $T_{\text{avail}}$), and every short cycle adds 4× transfer fees + interest to the harvest pool — they are the mechanism that grows $T_{\text{avail}}$ in the first place. Pre-unlock, the protocol funnels users toward shorts to build up the SOL float that ultimately unlocks longs.

Kani harness `verify_lending_gate_excludes_short_collateral` proves that the gate state depends only on $T_{\text{avail}}$, regardless of how much short collateral is parked. See [docs/lending-unlock.md](./lending-unlock.md).

---

## 5. Liquidation Threshold Analysis

### 5.1 When Does Liquidation Occur?

A position with initial collateral value $V_0$ and borrowed amount $b$ is liquidated when:

$$\text{LTV} = \frac{b + I}{V} > \theta$$

where $\theta = 6500$ bps (65%), $I$ is accrued interest, and $V$ is current collateral value.

Ignoring interest, the required price decline from position creation for liquidation is:

$$\frac{V}{V_0} < \frac{b}{\theta \cdot V_0} = \frac{\text{LTV}_0}{\theta}$$

Or equivalently, price must drop by a factor:

$$\delta > 1 - \frac{\text{LTV}_0}{\theta}$$

### 5.2 Depth Curve Alone

As the curve approaches its asymptote ($L_{\max} = 60\%$, very deep pool):

$$\delta > 1 - \frac{0.60}{0.65} = 1 - 0.923 = 0.077$$

A **7.7% price drop** triggers liquidation. This is the least conservative case (deep pool, maximum LTV used) — the price for the leverage a deep, hard-to-manipulate pool can safely support.

At the floor ($L_{\min} = 30\%$, 100 SOL pool):

$$\delta > 1 - \frac{0.30}{0.65} = 1 - 0.462 = 0.538$$

A **53.8% price drop** is required. Thin pools lever conservatively by construction. (At intermediate depth, e.g. 200 SOL → 45% LTV → **30.8% drop**; 500 SOL → 54% → **16.9% drop**.)

### 5.3 Per-User Cap Interaction

With $\mu = 23$, the per-user cap can impose a **stricter** effective LTV than the depth curve whenever a borrower's collateral is small relative to lendable SOL — graduating from protective (fresh tokens) to functional (mature tokens). The per-user mechanism is unchanged from V20; only the depth ceiling it competes with moved. From Section 3.5:

- Fresh treasury (22 SOL): per-user cap → 3% LTV → **95% drop** to liquidate
- Moderate treasury (150 SOL): per-user cap → 20% LTV → **69% drop** to liquidate
- Deep treasury (500 SOL pool): depth curve binds at 54% → **16.9% drop** to liquidate

### 5.4 Regime Map

| Regime | Binding Constraint | Typical LTV | Price Drop for Liquidation |
|--------|-------------------|-------------|---------------------------|
| Below floor (< 100 SOL pool) | Depth curve | 0% (no leverage) | — |
| Fresh treasury (< 50 SOL) | Per-user cap | 3% | 95% |
| Growing treasury (50-150 SOL) | Per-user cap | 7-20% | 69-89% |
| Mature treasury (150-500 SOL) | Per-user cap → depth curve | 20-54% | 17-69% |
| Deep treasury (500+ SOL pool) | Depth curve | 54-60% | 7.7-17% |

The per-user cap dominates whenever $M \cdot \mu \cdot y / (S \cdot x) < L(x) / 10000$. With $\mu = 23$, this transition occurs once the treasury is large relative to the pool — the point where the token has proven itself through sustained volume and the depth **curve** takes over as the safety ceiling. Orthogonally, **Rail 2** caps any single position's debt value at $\rho_{\max} = 25\%$ of pool SOL: on a thin pool a whale's borrow is clamped well below the LTV ceiling, holding the liquidation-unwind slippage to $\rho_{\max}$ regardless of which regime the position sits in.

### 5.5 Simulation Validation

The economic simulator ([`sim/torch_sim.py`](../sim/torch_sim.py)) is the executable spec for these rails, and the on-chain math is a faithful port of it. Key validations:

- **Rails scenario (`scenario_depth_rails`).** On pools from the 100-SOL floor up, a whale's leverage is clamped by both the LTV curve and the size cap; the realized debt-to-pool ratio pins to $\rho_{\max} = 25\%$ at *every* depth — confirming unwind slippage is depth-invariant. A capped position driven into genuine TWAP distress then liquidates with the flat bonus covering the unwind ($\rho \leq \rho_{\max} \leq$ bonus) → **zero protocol bad debt**.
- **Conservation fuzzers.** Two 1500-action random-walk scenarios (mixed open/close/liquidate across many users) hold SOL and token conservation at *every* step — bad debt is only ever redistribution, never invented or destroyed value.
- **Gap-crash stress.** When a price gap *outruns* liquidation (e.g. a one-block 75% crash), residual bad debt appears but is bounded by the single position's custody (the size cap + per-position vault), and conservation still holds. The rails bound unwind slippage; they cannot bound a price that teleports past the threshold faster than any liquidator can act — that tail is capped by custody, not eliminated.

The liquidation engine is not decorative — it functions exactly as designed when depth makes leverage real, and the rails ensure that what it cannot prevent, it bounds.

---

## 6. Long-Short Asymmetry

The structural near-impossibility of liquidation applies **only to long positions** (SOL borrowed against token collateral). Short positions (tokens borrowed against SOL collateral) do not share this property.

### 6.1 Why Longs Are Safe

A long borrower deposits tokens and receives SOL. Liquidation requires the token price to *fall*, reducing collateral value. In a constant-product pool, price falling means SOL leaving the pool. But:

- Price drops are bounded: a token's price cannot fall below zero
- Large drops require proportionally large sell volume
- The maximum possible drop is 100% (total value loss)

With effective LTV at 0.29%, even the theoretical maximum drop (100%) only barely breaches the liquidation threshold.

### 6.2 Why Shorts Are Vulnerable

A short seller deposits SOL and borrows tokens. Liquidation requires the token price to *rise*, increasing the SOL value of the token debt. Price increases are **unbounded** — a token can 2x, 10x, or 100x.

The debt value for a short position is:

$$D = \frac{n \cdot x}{y}$$

where $n$ is tokens borrowed. As price rises ($x/y$ increases), $D$ grows without bound. A 3x price increase triples the debt value, pushing a 50% LTV short to 150% — deep into liquidation.

### 6.3 Formal Comparison

For a long position at initial LTV $\ell_0$, the price must drop by $\delta$ for liquidation:

$$\delta_{\text{long}} > 1 - \frac{\ell_0}{\theta}$$

For a short position at initial LTV $\ell_0$, the price must rise by factor $\alpha$ for liquidation:

$$\alpha_{\text{short}} > \frac{\theta}{\ell_0}$$

At $\ell_0 = 0.50$ and $\theta = 0.65$:
- **Long:** needs a 23% drop (bounded, requires real sell pressure)
- **Short:** needs a 1.3x pump (unbounded, common in volatile markets)

At $\ell_0 = 0.0029$ (per-user cap dominated):
- **Long:** needs a 99.6% drop (near-impossible)
- **Short:** needs a 224x pump (extremely unlikely but theoretically possible)

### 6.4 Design Implication

This asymmetry is correct and intentional. Borrowing SOL against tokens you hold (long) is a bet that the token retains some value — a conservative position. Shorting is a bet that the token will decline — an inherently riskier directional trade.

The protocol reflects this: long positions are structurally protected by the cap interaction. Short positions are protected by the depth curve and per-user caps, but remain liquidatable under adverse price movement. The liquidation mechanism exists primarily to service short positions.

---

## 7. Transfer Fee as Treasury Growth Engine

Treasury SOL accumulates from two sources: bonding curve fee splits (pre-migration) and transfer fee harvesting (post-migration). Both are significantly more productive than naive models suggest.

### 7.1 Pre-Migration: PVP Bonding Multiplier

During bonding, each buy contributes a dynamic treasury share (17.5% at start, decaying to 2.5% at completion). The naive model assumes one-way buying to target — e.g., 200 SOL of buys for a Torch-tier bond.

In practice, bonding curves exhibit heavy PVP (player vs player) trading: traders buy, take profit, re-enter, panic sell, etc. Empirical observation on Pyre.world (built on Torch) shows tokens reaching 80+ SOL in treasury at only 50% bonded — implying gross buy volume of 5-10x the net curve progression.

The treasury contribution from a single buy at reserves $r$ with amount $a$ is:

$$T_{\text{buy}} = a \cdot (1 - \frac{f_p}{10000}) \cdot \frac{\text{rate}(r)}{10000}$$

where $f_p = 50$ bps (protocol fee) and $\text{rate}(r)$ decays from 1750 to 250 bps. When the same SOL cycles through buys and sells multiple times, the treasury captures the rate on every buy. A 3x volume multiplier (common for active tokens) yields roughly 3x the treasury SOL at migration.

| Volume Multiplier | Estimated Treasury at Migration (Torch tier) |
|-------------------|----------------------------------------------|
| 1x (no PVP) | ~22 SOL |
| 2x | ~35 SOL |
| 5x | ~70 SOL |
| 10x (heavy PVP) | ~100+ SOL |

### 7.2 Post-Migration: Transfer Fee Harvesting

Every token transfer incurs a 0.07% fee ($f = 7$ bps) via the Token-2022 extension. This fee is:
1. Withheld from the transfer amount (in tokens)
2. Accumulated in the token mint
3. Harvested to the treasury token account (permissionless)
4. Swapped to SOL via the pool

The naive linear model assumes constant token price:

$$\dot{T}_{\text{naive}} = V_d \cdot \frac{f}{10000}$$

This significantly underestimates real treasury growth because **transfer fees are collected in tokens, and token value fluctuates**. The correct model:

$$\dot{T} = \sum_i \frac{f \cdot n_i}{10000} \cdot P(t_h)$$

where $n_i$ is the token amount transferred in trade $i$, and $P(t_h)$ is the token price at harvest time — not at transfer time. Since harvesting is batched and permissionless, the treasury benefits from price appreciation between transfers and harvest.

### 7.3 Price-Volume Correlation

Price and volume are positively correlated — pumps drive both higher. During a pump:
- More trades occur (higher $n_i$)
- Each trade transfers more tokens (larger position sizes)
- Accumulated fee tokens are worth more at harvest ($P(t_h)$ elevated)

This creates a multiplicative effect. A 4x200 SOL buy sequence might generate 320K tokens in fees. If price has doubled during the sequence, those tokens harvest for 2x the naive estimate.

**Empirical example:** 4 buys of 200 SOL on Pyre.world generated ~80K tokens in fees, harvesting for 1.2 SOL in a single swap — because price was elevated at harvest time.

### 7.4 Compounding Effect

Treasury growth compounds through three reinforcing loops:

$$T(t+1) = T(t) + H(t) + I(t) + B(t)$$

where:
- $H(t)$ = SOL from harvest-and-swap (transfer fee conversion)
- $I(t)$ = interest collected from active loans
- $B(t)$ = bonding fee accumulation (pre-migration only)

As treasury grows, more SOL is lendable. More lending generates more interest. More interest deepens the treasury. The lending pool is self-funding and self-accelerating.

### 7.5 Community Token Model

For community tokens (no creator fee split), 100% of harvested fees and bonding splits flow to treasury. The protocol extracts nothing post-migration. All value generated by trading activity stays within the token's ecosystem.

---

## 8. Short Pool Stability

The 300M-token `TreasuryLock` is the short pool: tokens lent out to shorters and returned on close. Token-2022 imposes a 0.07% transfer fee on every transfer ($f = 7$ bps). The protocol must keep the lock balance stable across cycles regardless of hold duration — otherwise short cycles below the interest-coverage threshold would slowly drain the pool.

### 8.1 Record-Gross Design

The lock conservation property requires that the **gross amount sent from the lock** equals the **debt the borrower owes back** — not the net the borrower received. So on open, `position.tokens_borrowed = args.tokens_to_borrow` (gross), even though the shorter's wallet only credits with $\text{gross} - \lceil \text{gross} \cdot f / 10000 \rceil = \text{net}$ after the Token-2022 withhold.

This makes the borrower responsible for the open-leg fee gap: they received $\text{net}$ tokens at open, but owe back $\text{gross}$ at close (plus interest). The gap is funded by buying tokens on the DEX before close, or out of any wallet balance they otherwise hold. The economic cost is real (~0.07% on the round-trip vs. ~0.07% under the old net-recording design — same total) but it is now visible to the borrower as a fee they pay at close, rather than absorbed silently by the lock.

### 8.2 Gross-Up Accounting on Close

On every short close and liquidation, the borrower (or liquidator) sends a grossed-up amount so the lock receives the full intended net:

$$\text{gross}_\text{close} = \left\lceil \frac{(p + i) \cdot 10000}{10000 - f} \right\rceil$$

The Token-2022 withhold removes $\lceil \text{gross}_\text{close} \cdot f / 10000 \rceil$, and the lock's ATA credits with $p + i$. Kani harness `verify_gross_up_preserves_net_delivery` (`kani_proofs.rs`) proves the recipient never receives less than the target net and the overshoot is bounded by 1 unit.

### 8.3 Per-Cycle Lock Balance

For a short cycle with gross-recorded principal $p$ and interest $i$ accrued, applied to a lock balance $L_0$:

| Step | Transfer | Withhold | Lock Δ |
|---|---|---|---|
| Open | $p \to $ shorter | $\lceil pf/10000 \rceil$ | $-p$ |
| Close | shorter $\to L$ ($\text{gross}_\text{close}$) | $\lceil \text{gross}_\text{close} \cdot f/10000 \rceil$ | $+p + i$ |
| Cycle total | | | $+i$ |

The lock is **strictly non-decreasing across any complete cycle**, with the net change being exactly the interest paid (modulo ±1 from the gross-up ceiling). Zero-interest cycles (open and close in the same slot) leave the lock unchanged, not negative. This holds regardless of hold duration — Kani harness `verify_short_full_close_lock_conservation` proves the invariant.

### 8.4 Borrower Round-Trip Cost

For a short of size $p$ tokens held over interest $i$, the borrower pays:

- Open leg: 0 tokens out-of-pocket (lock pays the open transfer fee implicitly by sending gross)
- Close leg: $\text{gross}_\text{close} - \text{net\_received\_at\_open} \approx p \cdot 0.14\% + i \cdot 1.0007$

In effect, the borrower funds the *entire* round-trip transfer-fee cost (~0.14%) plus the interest with its gross-up. Compared to the prior net-recording design where the lock absorbed the open-leg fee, this is the same total fee burden — just allocated to the borrower rather than to protocol pool drain. The economic value is preserved (fees still flow to harvest → SOL treasury), and the lock balance is now a conservation invariant rather than a soft floor.

### 8.5 Dynamic Cap Base

`check_short_caps` reads the *current* `treasury_lock_token_account.amount` as the cap base for the global short utilization (80% of lock balance). As the lock grows from interest, the short capacity grows with it — a self-reinforcing loop where successful protocol operation expands future capacity.

---

## 9. Invariants

The following properties hold at all times:

**I1: Supply conservation.** Total token supply is exactly $S = 10^{15}$. No minting occurs post-creation. Mint authority is revoked at migration.

**I2: Pool invariant.** $k' \geq k$ after every swap. $k$ is non-decreasing (no liquidity removal; LP tokens are burned at migration).

**I3: Treasury solvency.** $T_{\text{avail}} \geq \sum b_i \cdot (1 - U/10000)$. At least 20% of available SOL is always unlent.

**I4: Position isolation.** Each user has at most one `LoanPosition` and one `ShortPosition` per token. No cross-collateralization exists.

**I5: Depth monotonicity.** $L(x_1) \leq L(x_2)$ for $x_1 \leq x_2$. Deeper pools always permit equal or higher LTV.

**I6: Cap independence.** The per-user borrow cap $B_{\max}(c)$ is independent of other users' positions. One user's borrow does not affect another user's cap (only the global utilization ceiling creates interaction).

**I7: Gate independence from short collateral.** The lending unlock gate compares $T_{\text{avail}} = T_{\text{sol}} - T_{\text{short}}$ against the threshold. Opening or closing any short position adds and removes equal amounts to both $T_{\text{sol}}$ and $T_{\text{short}}$, leaving $T_{\text{avail}}$ unchanged. The gate state depends only on protocol-earned float (Kani: `verify_lending_gate_excludes_short_collateral`).

**I8: Short pool conservation.** The `TreasuryLock` token balance is non-decreasing across any complete short cycle (open + close), regardless of hold duration. Net change per cycle is exactly the interest paid (modulo ±1 from gross-up ceiling). Zero-interest cycles leave the balance unchanged, not negative. Proven by `verify_short_full_close_lock_conservation`.

---

## 10. Attack Analysis

### 10.1 Price Pump and Borrow

**Attack:** Buy tokens to inflate price, borrow SOL at inflated collateral value, let price revert.

**Defense:** The attacker must spend $\Delta x = x(\sqrt{\alpha} - 1)$ SOL to pump price by $\alpha$. At a 200 SOL pool, a 20% pump costs ~19 SOL. The attacker receives tokens worth $\Delta x$ SOL at the inflated price.

Even at the 200-SOL pool's max-curve LTV (45%), the attacker can borrow at most $0.45 \cdot \Delta x \approx 8.6$ SOL against those tokens. Their net cost is $\Delta x - 0.45 \cdot \Delta x = 0.55 \cdot \Delta x$. They lose money. (Rail 2 caps the borrow further still — at most $\rho_{\max} = 25\%$ of pool SOL — if the pumped collateral is large relative to depth.)

**With per-user cap:** The attacker's maximum borrow is further limited to $B_{\max}(c) \ll 0.5 \cdot V(c)$, making the attack strictly unprofitable.

### 10.2 Price Dump and Liquidation Hunting

**Attack:** Sell tokens to crash price, liquidate other users' positions, collect bonus.

**Defense:** With effective LTV at 0.29% (per-user cap dominated), a 99.6% price crash is needed. This would require the attacker to sell enough tokens to remove 99.6% of pool SOL — approximately the entire pool. The attacker would receive far less SOL than they spend in tokens due to constant-product slippage.

### 10.3 Sybil Borrowing

**Attack:** Use many wallets to circumvent per-user cap.

**Defense:** Each wallet needs real token collateral. Total borrowing across all sybil wallets is still bounded by the global utilization cap ($0.8 \cdot T_{\text{avail}}$). The per-user cap prevents any single wallet from taking a disproportionate share, but the utilization cap is the hard ceiling regardless. The absolute 20%-of-lendable per-user clamp ($\beta = 2000$ bps) means even a whale must split across at least five wallets to monopolize lending, and each wallet pays its own gas + rent.

### 10.4 Interest Accrual Liquidation

**Attack:** Open position, wait for interest to push LTV past liquidation threshold.

**Analysis:** Interest accrues at $r = 150$ bps per epoch (~7 days; [V21] lowered from 200). Starting at 0.29% LTV:

$$\text{Epochs to liquidation} = \frac{(\theta - \text{LTV}_0)}{r} = \frac{(6500 - 29)}{150} = 43.1 \text{ epochs} \approx 302 \text{ days}$$

The borrower has roughly 10 months to repay before interest alone triggers liquidation. At the depth curve's asymptote (60% LTV, deep pool — the most aggressive case):

$$\text{Epochs} = \frac{(6500 - 6000)}{150} = 3.3 \text{ epochs} \approx 23 \text{ days}$$

At the 100-SOL floor (30% LTV) it is $(6500 - 3000)/150 = 23.3$ epochs $\approx 163$ days. Interest-only liquidation is therefore a fast clock *only* for a borrower who deliberately maxes leverage on a deep pool, and assumes zero favorable price movement.

### 10.5 Gate Bypass via Short Collateral

**Attack:** Open a 100 SOL short on a fresh post-migration token to inflate `treasury.sol_balance` past the 100 SOL lending gate, then borrow against the freshly "unlocked" pool.

**Defense:** The gate compares $T_{\text{avail}} = T_{\text{sol}} - T_{\text{short}}$ against the threshold (see §4.3). Opening a short of size $s$ increments both $T_{\text{sol}}$ and $T_{\text{short}}$ by $s$, leaving $T_{\text{avail}}$ unchanged. The gate state is invariant under any short collateral movement — Kani-proven by `verify_lending_gate_excludes_short_collateral`. This was a real semantic bug in an earlier v20 build (V20C-1) and is now closed.

---

## 11. Comparison to Traditional DeFi Lending

| Property | Torch Market | Aave/Compound |
|----------|-------------|---------------|
| Price oracle | Pool reserves (on-chain) | Chainlink (off-chain) |
| Maximum LTV | 0.29-60% (regime dependent) | 75-85% |
| Liquidation frequency | Near-zero (structural) | Regular (by design) |
| Liquidator dependency | Minimal | Critical |
| Capital efficiency | Low (safety-first) | High (leverage-first) |
| Treasury funding | Self-funded (transfer fee) | External (governance) |
| Admin keys | None post-migration | Governance multisig |
| Cross-collateral | No | Yes |

The fundamental difference: traditional DeFi lending maximizes capital efficiency and relies on active liquidation to maintain solvency. Torch Market minimizes the probability of liquidation by constraining leverage at the protocol level. This trades capital efficiency for systemic safety.

---

## 12. Conclusion

The depth-anchored risk model creates a lending system where:

1. **Rail 1** — Maximum LTV adapts to pool manipulation resistance on a continuous, concave curve (30% at the 100-SOL floor → 60% asymptote, no stored state)
2. **Rail 2** — A per-position size cap ($\rho_{\max} = 25\%$ of pool SOL) makes liquidation-unwind slippage depth-invariant
3. **Rail 3** — A single flat liquidation bonus (32.5%), derived from the size cap, clears the unwind on every pool with no depth-varying schedule
4. Per-user caps ($\mu = 23$) create graduated leverage — 3% at fresh treasury, scaling toward the 60% depth-curve asymptote — and can bind stricter than the curve on a small treasury
5. An absolute 20%-of-lendable per-user clamp ($\beta = 2000$ bps) ensures no single whale can monopolize lending
6. The activity unlock gate ($T_{\text{avail}} \geq 100$ SOL on mainnet) keys on protocol-earned float, not gross balance — short collateral cannot cosmetically unlock the gate
7. Fresh tokens are structurally protected; mature tokens graduate into functional margin markets
8. Treasury grows perpetually from transfer fees (PVP bonding multiplier + price-correlated harvesting) without protocol extraction
9. The 300M short pool is preserved (and grows) across cycles via Token-2022 gross-up accounting
10. The liquidation mark is a keeperless TWAP from the pool's own price cumulative — no external oracle, keeper, or governance required
11. The liquidation engine is functional and validated — not decorative

The result is a permissionless, self-sustaining lending protocol with a natural lifecycle: tokens begin with near-zero liquidation risk (per-user cap dominance) and graduate into real margin markets as depth proves sustained demand. Short positions remain liquidatable at any stage due to the asymmetric nature of upward price risk.

The safety of the system is not a parameter choice — it is a mathematical consequence of the supply split ($y/S \approx 0.15$), the constant-product invariant, the depth-scaled rails, and the cap interactions. These properties are immutable post-deployment and hold for all valid inputs, as verified by Kani proof harnesses (including the depth-curve, size-cap, and bonus-ramp proofs) and economic simulation under adversarial conditions.
