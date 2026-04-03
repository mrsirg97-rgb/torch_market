# torch.market

**Every token is its own margin market.**

Brightside Solutions, 2026

[torch.market](https://torch.market) | [docs](https://torch-market-docs.vercel.app/) | [audit](https://torch.market/audit.md) | [@torch_market](https://x.com/torch_market/)

---

Launch a token on torch and it comes with a funded treasury, a 300M token lending reserve, margin lending, short selling, and on-chain pricing — all live from the moment it migrates to DEX.

No external liquidity providers. No oracle feeds. No protocol token. No bootstrapping period. The token funds its own financial infrastructure from its own activity.

This is not a launchpad. This is a protocol that turns every token into a self-contained financial system.

---

## What Torch Offers

- **Every token gets its own margin market at creation.** Not after bootstrapping. Not after governance votes. At creation. The treasury is the lender. The 300M token lock is the short pool. The DEX pool is the price feed.

- **Shorts are real, not synthetic.** Short sellers borrow real tokens from the treasury lock, sell them on the real market, and buy them back to close. This is not a perpetual contract. There is no funding rate. No mark price. Shorts contribute to price discovery because they ARE market participants.

- **All parameters are immutable.** 50% max LTV. 65% liquidation threshold. 2% interest per epoch. 10% liquidation bonus. 80% utilization cap. Set at deployment. No admin key can change them. You can read every parameter, calculate every outcome, and know the rules won't change after you open a position.

- **One user cannot collapse the pool.** Per-user borrow caps (5x collateral share of supply), utilization caps (20% of treasury always reserved), and isolated positions (one loan per user per token) mean the system stays available for everyone regardless of what any single actor does.

- **Circuit breakers protect against manipulation.** New margin positions are blocked when pool liquidity drops below 5 SOL or price deviates more than 50% from migration baseline. Liquidations remain functional (safety valve) but require minimum liquidity. No off-chain keepers, no oracles — the program reads pool state directly and refuses to act when the price signal is untrustworthy.

- **No rug pull is structurally possible.** Mint and freeze authority are revoked permanently at creation. LP tokens are burned. Liquidity is locked forever. Not by promise — by code.

- **Creator economics are immutable.** Creators choose at launch: community token (100% of fees to treasury, zero extraction) or creator token (85/15 split). The default is community. The choice is permanent.

## What Torch Does Not Offer

- **High leverage.** Max is 2x per position. Sophisticated users can compose higher effective leverage through recursive borrowing — deposit tokens, borrow SOL, buy more tokens, borrow again — but the protocol doesn't hand you 20x on a memecoin.

- **No loss.** Individual positions can be liquidated. In extreme conditions, a position can accumulate bad debt. But bad debt is isolated — it cannot spread. One position going underwater does not affect anyone else's collateral, LTV, or access to the lending pool. No cascading liquidations. No contagion. Your position's outcome is yours. Nobody else's failure makes your position unhealthy.

- **Unlimited liquidity.** The lending pool is whatever the treasury has earned. The short pool is 300M tokens plus accrued interest. Real capital, real limits.

---

## How It Works

A token on torch goes through four phases. Each phase builds the next. By the time margin activates, every piece of infrastructure is already in place.

### Phase 1: Bonding

A new token launches with a constant-product bonding curve. Users buy tokens with SOL.

Total supply is 1B tokens. **700M (70%)** enter the bonding curve for sale. **300M (30%)** are locked in the treasury lock from creation — this is the short selling pool.

**100% of tokens purchased go to the buyer.** No splits, no vote vaults, no governance overhead.

SOL from each buy is split:

| Destination | Rate |
|-------------|------|
| Bonding curve | 82% → 97% (grows as bonding progresses) |
| Token treasury | 17.5% → 2.5% (decays as bonding progresses) |
| Protocol treasury | 0.5% (90% rewards, 10% dev) |
| Creator wallet | 0% → 1% (creator tokens only; community tokens: 0%) |

Bonding completes at 100 SOL (Flame) or 200 SOL (Torch). Every wallet is capped at 2% of supply during bonding.

### Phase 2: Migration

When bonding completes, anyone can trigger migration. No creator, no admin, no single party can block it.

The protocol creates a Raydium CPMM pool with the curve's SOL and remaining tokens, **burns all LP tokens** (liquidity locked forever), activates the 0.04% transfer fee, and **revokes mint and freeze authority permanently**.

### Phase 3: Trading

Post-migration, the token trades on Raydium. The 0.04% transfer fee collects on every transfer — wallet to wallet, DEX swaps, everything. Anyone can harvest these fees and swap them to SOL, growing the treasury.

The harvest cycle:

```
Every transfer → 0.04% withheld → harvest to treasury → swap to SOL
    │
    └── Community tokens: 100% to treasury
        Creator tokens:   85% to treasury, 15% to creator
```

This is perpetual. As long as the token moves, the treasury grows.

### Phase 4: Margin

This is the payoff. The token now has two capital pools on-chain:

| Pool | Asset | Source | Purpose |
|------|-------|--------|---------|
| **Token Treasury** | SOL | Fees + interest | Lending pool — borrow SOL against token collateral |
| **Treasury Lock** | 300M tokens | Locked at creation | Short pool — borrow real tokens against SOL collateral |

**Lending:** Deposit tokens as collateral. Borrow SOL from the treasury. Use it however you want. Repay SOL + interest to unlock your collateral. If your position's LTV exceeds 65%, anyone can liquidate it.

**Short selling:** Deposit SOL as collateral. Borrow real tokens from the treasury lock. Sell them on the market. If the price drops, buy back cheaper, return the tokens + interest, keep the difference. If the price rises past 65% LTV, anyone can liquidate.

Both sides use the same parameters. Both are overcollateralized. Both are isolated per user per token. Both are self-replenishing — borrowers pay SOL interest back to the treasury, short sellers pay token interest back to the lock.

| Parameter | Value |
|-----------|-------|
| Max LTV | 50% |
| Liquidation threshold | 65% |
| Interest rate | 2% per epoch (~7 days) |
| Liquidation bonus | 10% |
| Utilization cap | 80% |
| Per-user cap | 5x collateral share of supply |
| Liquidation close | 50% per call |
| Min pool liquidity | 5 SOL (blocks all margin ops below this) |
| Max price deviation | 50% from baseline (blocks new positions only) |

---

## Why This Is Different

The typical path to margin infrastructure:

| | What you need |
|---|---|
| **Aave/Compound** | External LPs, governance token, Chainlink oracle, incentive programs, months of TVL growth |
| **CEX margin** | Centralized lender, counterparty risk, KYC |
| **Perp DEXs** | LP pool to take the other side, oracle infrastructure, insurance fund, funding rate mechanism |

On torch:

| Component | Source |
|-----------|--------|
| Lending pool | Token treasury — funded by its own fees |
| Short pool | Treasury lock — 300M tokens locked at creation |
| Price feed | Raydium pool reserves — created at migration |
| Liquidation | Permissionless — anyone can call |
| Recapitalization | 0.04% transfer fee — perpetual |

Nothing external. Nothing to bootstrap. Nothing to incentivize. The token IS the margin market.

### Why Not Perps

Perpetual futures are synthetic. You trade a contract that tracks a price via a funding rate mechanism. Nobody borrows anything. Nobody sells anything real. The underlying doesn't move. When the oracle is wrong, everyone gets liquidated against a number that doesn't reflect reality.

On torch, shorts borrow real tokens and sell them on the real market. That sell moves the real price. When they close, they buy real tokens back. Short sellers are market participants contributing to price discovery — not placing side bets on a synthetic feed.

There is no oracle to manipulate. There is no funding rate to spike. There is no LP pool praying it stays solvent. The counterparty is 300M tokens that were locked at creation for exactly this purpose.

Most perps protocols have a single point of failure: the off-chain price feed. If the keeper stops cranking, the oracle goes stale, and either liquidations halt (bad debt accumulates) or positions get liquidated against a stale price (users get robbed). Torch has zero off-chain dependencies — the program reads Raydium pool state directly, and when pool conditions are unhealthy, circuit breakers refuse to open new positions rather than acting on bad data.

---

## Recovery

Not every token succeeds. Torch handles failure.

**Reclaim:** If a token hasn't completed bonding and is inactive for 7+ days, anyone can reclaim it. All SOL moves to the protocol treasury and gets distributed to active traders as epoch rewards. Failed tokens become other people's rewards.

**Revival:** A reclaimed token can be revived if the community funds it back to threshold (37.5 SOL Flame / 75 SOL Torch). Contributors get no tokens — they're signaling belief. When the threshold is reached, trading resumes automatically.

---

## Agents

There is no API server. The SDK builds transactions locally from the on-chain program's Anchor IDL. Agents read state directly from Solana RPC. No middleman, no API keys, no trust assumptions beyond the program itself.

```
Agent → SDK (local) → Solana RPC → On-chain program
```

The Torch Vault is a protocol-native custody layer. A vault-linked agent can trade, lend, and short — all without holding tokens or significant SOL in its own wallet. If the agent's key is compromised, the attacker gets dust. The vault authority unlinks and re-links.

Every parameter is readable on-chain. Every outcome is calculable before execution. Deterministic rules, composable primitives, no hidden state. This is infrastructure that agents can reason about.

---

## Verification

70 Kani proof harnesses. 52 end-to-end tests. All passing. Cross-validated by independent audit (OpenAI o3).

Core arithmetic is formally verified with [Kani](https://model-checking.github.io/kani/) covering every possible input in constrained ranges: fee calculations, bonding curve pricing, lending formulas, liquidation lifecycle, short selling, bad debt accounting, circuit breaker band math, reward distribution, migration conservation, token distribution. No SOL created from nothing. No tokens minted from thin air. No fees exceeding stated rates.

See [VERIFICATION.md](https://torch.market/verification.md).

---

## Constants

| Parameter | Value |
|-----------|-------|
| Total supply | 1,000,000,000 (6 decimals) |
| Curve supply | 700,000,000 (70%) |
| Treasury lock | 300,000,000 (30%) |
| Max wallet (bonding) | 2% of supply |
| Bonding target | 100 SOL (Flame) / 200 SOL (Torch) |
| Protocol fee | 0.5% (bonding only) |
| Treasury SOL share | 17.5% → 2.5% (dynamic decay) |
| Creator SOL share | 0% → 1% (creator tokens only) |
| Transfer fee | 0.04% (post-migration, immutable) |
| Fee swap split | 100% treasury (community) / 85-15 treasury-creator |
| Max LTV | 50% |
| Liquidation threshold | 65% |
| Interest rate | 2% per epoch (~7 days) |
| Liquidation bonus | 10% |
| Utilization cap | 80% |
| Per-user borrow cap | 5x collateral share of supply |
| Min pool liquidity | 5 SOL |
| Max price deviation | 50% from migration baseline |
| Min borrow | 0.1 SOL |
| Epoch duration | ~7 days |
| Reward eligibility | 2 SOL volume per epoch |
| Inactivity reclaim | 7 days |
| Revival threshold | 37.5 SOL (Flame) / 75 SOL (Torch) |

---

*© 2026 Brightside Solutions. All rights reserved.*

[Terms](https://torch.market/terms) | [Privacy](https://torch.market/privacy) | [torch.market](https://torch.market)
