# Lending Unlock (Treasury-Gated)

Not implemented. Design captured for execution.

## Problem

Today, lending opens immediately on every token. With a fresh treasury holding ~13
SOL at graduation, a typical 1M-token holder can borrow at most **0.24 SOL** against
their position. That ratio (0.24 SOL borrow against ~$2.8k of collateral at typical
launch prices) is so absurd that lending is functionally dead-on-arrival for the
average user.

The root cause is mechanical: per-user borrow cap is `max_lendable × user_collateral
× 23 / TOTAL_SUPPLY`, clamped at `max_lendable × 20%`. With `max_lendable = treasury
× 80%`, the per-user cap is dominated by treasury size. A small treasury → tiny
per-user cap → useless lending UX.

Two options:
1. **Loosen the per-user cap** when treasury is small. Awful: creates whale-drain
   risk that we just hardened against in the 20% absolute ceiling fix.
2. **Defer lending entirely until treasury crosses a meaningful threshold.** Wait
   for the protocol's own activity to fund lending into a useful capacity range,
   then unlock.

This document specifies option 2.

## The deeper design property

Lending unlock is the protocol's **volume-based IQ test for tokens**. Every other
lending market on Solana gates capacity on PRICE (via oracles). Torch gates capacity
on PROTOCOL ACTIVITY — measured by treasury SOL balance, which grows ONLY from
real economic events:

- Bonding-curve trade fees
- Migration / DEX swap fees
- Token-2022 transfer fees on every token movement (wallet-to-wallet, sell, short
  open, short close — 0.07% per transfer, paid into treasury and periodically
  swapped to SOL via `swap_fees_to_sol`)
- Short interest accrued per epoch

Price can be pumped without paying for it. Volume cannot.

The downstream consequences:

- **Graduation ≠ lending unlock.** Bonding curve completes at the configured
  sol_target (typically 100 SOL for flame). Lending unlocks at a separate, treasury-
  measured threshold (proposed 100 SOL). Same number in some configurations, but
  conceptually decoupled — they're different timers measuring different things.
- **A token can graduate fast but never unlock lending.** One-shot pump → curve
  completes → volume dies → treasury never crosses threshold → lending stays
  locked forever. Correct outcome: that token did not earn the secondary feature.
- **A token can take longer to graduate but unlock lending quickly.** Sustained
  community trading → fees compound → treasury crosses threshold before graduation
  even completes. Also correct: this token earned it.
- **The shorts mechanic is the bootstrap.** Each short generates 4× transfer fees
  per cycle, plus interest, plus slippage to the curve. Shorts grow the treasury
  faster than passive holding. The protocol's value flow is:
  `shorts → transfer fees → treasury SOL → lending unlock → leverage compounds`.

## Threshold and capacity numbers

Treasury SOL → max_lendable (80%) → per-user cap (20% absolute) for a typical
1M-token holder (per-user formula cap is `max_lendable × 0.001 × 23`):

| Treasury SOL | max_lendable | 1M-token user can borrow | 76M-token user can borrow (= 20% absolute cap) |
|---|---|---|---|
| 13 SOL (today, at graduation) | 10.4 | **0.24 SOL** ❌ useless | 2.08 SOL |
| 50 SOL | 40 | 0.92 SOL | 8 SOL |
| **100 SOL (proposed threshold)** | 80 | **1.84 SOL** ✓ usable | 16 SOL |
| 250 SOL | 200 | 4.6 SOL | 40 SOL |
| 500 SOL | 400 | 9.2 SOL | 80 SOL |
| 1000 SOL | 800 | 18.4 SOL | 160 SOL |

100 SOL is the threshold where lending becomes genuinely useful for both the
average retail holder AND the whale.

## Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | **Gate on `treasury.sol_balance`, not pool depth or curve completion.** | Treasury SOL is the *output* of all volume-driven activity. Decoupled from any single signal. |
| 2 | **Threshold = `MIN_TREASURY_SOL_FOR_LENDING = 100 SOL`.** | Numbers above. Tunable later via admin if observation says different. |
| 3 | **No flag, no one-way logic. Just `treasury.sol_balance >= MIN`.** | `sol_balance` is the treasury's principal pool — it tracks `liquid_lamports + total_sol_lent` conceptually, not the live PDA lamport balance. Normal borrow/repay moves lamports but does NOT change `sol_balance`; only deposits, withdrawals, interest accrual, and bad-debt write-downs do. So once `sol_balance >= 100`, it stays >= 100 unless admin withdraws or a liquidation goes bad. Re-locking IS the correct behavior in those rare cases — the admin intentionally reduced capacity, or the protocol took a real loss. No need to engineer hysteresis around something that doesn't naturally fluctuate. |
| 4 | **No gradient ramp.** Discrete unlock at 100 SOL. | Simpler. Discrete is also a better marketing event (token unlocks lending, milestone moment). Gradient = no clear celebration. |
| 5 | **Shorts unaffected.** Shorts continue working from launch regardless of treasury SOL — they borrow from treasury *tokens*, not SOL. The unlock gate applies only to long lending (borrow SOL against tokens). | Phase 1 (always): launchpad + shorts. Phase 2 (post-threshold): + lending. Shorts are the bootstrap mechanism; locking them would break the funding flywheel. |
| 6 | **Surface lending status prominently in UI.** Detail page, home dashboard, optionally global activity feed when a token crosses the threshold. | Makes unlock a visible milestone, like bonding-curve progress. Drives creator/holder engagement with promoting volume. |

## Implementation outline

Estimated ~10 lines on-chain + frontend status copy. The principal-vs-liquid
accounting model in Treasury means we don't need state changes or new events —
just a runtime check.

**1. On-chain (`programs/torch_market`)**

```rust
// constants.rs
pub const MIN_TREASURY_SOL_FOR_LENDING: u64 = 100_000_000_000; // 100 SOL

// handlers/lending.rs::check_borrow_caps — first check:
require!(
    treasury.sol_balance >= MIN_TREASURY_SOL_FOR_LENDING,
    TorchMarketError::LendingNotYetUnlocked,
);

// errors.rs
#[msg("Lending unlocks once treasury reaches the activity threshold")]
LendingNotYetUnlocked,
```

That's it. No new event, no state field, no flip logic. The gate naturally
stays open once crossed because `sol_balance` doesn't fluctuate with normal
lending activity; and if it ever drops below threshold (admin withdrawal or
bad debt), the gate correctly re-engages.

**2. Frontend**

- Lending tab on token detail: when `treasury.sol_balance < MIN`, show status
  "Lending unlocks at 100 SOL · Current: 72 SOL" with a progress bar. Hide /
  disable the borrow button.
- The indexer already broadcasts `Market` frames on each trade that include
  the relevant treasury accounting; frontend just reads the value and renders.
  No new wire format needed.
- Activity feed milestone ("$TOKEN unlocked lending") is deferred — see
  Deferred section below.

## Deferred (worth doing later, not now)

- **`LendingUnlocked` milestone event + activity feed entry.** Currently the
  unlock is silently visible in the UI (progress bar fills, lending becomes
  active). To make crossings a discoverable social event ("$TOKEN unlocked
  lending — 100 SOL of activity"), add a one-shot `lending_unlocked: bool`
  flag on Treasury purely for event dedup, plus an `emit_cpi!(LendingUnlocked)`
  in the borrow handler at first-crossing. The lending GATE stays just
  `sol_balance >= MIN`; the flag exists only so the event fires once. Layer
  on whenever activity-feed becomes a UX priority.
- **Configurable threshold per market.** Maybe some tiers (spark/flame/torch)
  have different thresholds because their fee accumulation rates differ. For now,
  one constant for all.
- **Gradient capacity.** Ramp from 0% to 100% utilization across a range of
  treasury values. Adds dial complexity for marginal UX gain.
- **Notification / push events.** Surfacing milestone crossings on social /
  email / push. Depends on the milestone event above being wired first.
- **Per-mint lending toggle (admin override).** Force-unlock a market for
  promotional reasons, or force-lock if abuse detected. Not needed v1.

## Tripwires (when to revisit)

- If observation shows >50% of tokens never unlock lending → threshold too high.
- If newly-unlocked tokens are immediately drained by whales using the 20%
  per-user cap → consider raising the absolute ceiling or adding cooldown.
- If `LendingUnlocked` events are spamming activity feeds at scale → batch /
  rate-limit the frontend display, not the on-chain emission.

## When NOT to drop this

The threshold gate is load-bearing for the protocol's identity ("volume-earned
features"). Don't relax it under user pressure to "just open lending faster" —
the friction is the feature. Tokens that don't earn the unlock weren't supposed
to have lending.
