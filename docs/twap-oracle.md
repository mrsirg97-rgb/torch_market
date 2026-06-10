> **Mirrored from `deep_pool/docs/twap-oracle.md`.** deep_pool is the canonical
> owner of the oracle mechanism (the TWAP lives in the AMM so it's oracle-less).
> This copy keeps torch's liquidation-mark docs self-contained. **If you change
> the oracle, edit deep_pool's copy first, then re-sync this one.** Last synced
> from deep_pool @ 2026-06-01.

# TWAP oracle (in-pool, keeperless)

## Goal

deep_pool maintains its own time-weighted price oracle, updated on every swap.
Consumers (torch_market liquidations) read a manipulation-resistant mark with
**no keeper**: in a CPMM the marginal price changes *only* on swaps, so an
accumulator advanced on the swap itself never misses a price move — there is
nothing to sample between swaps. This relocates torch's existing observation
ring from `Treasury` to the layer that owns the price, and deletes torch's
`record_observation` crank (the keeper).

**Status:** deep_pool side implemented + green (25 kani, 7 proptests, 2 litesvm).
torch consumption done (V21 closed-loop leverage — liquidations read this mark).

## Model

V3-*shaped* (ring of history, the pool answers a **caller-chosen** lookback —
`observe(secondsAgo)` style) but a plain
**price cumulative** — the canonical AMM oracle (Uniswap `price0`/`price1`), what
an arbitrary integrator expects. We accumulate `Σ price_q64 × Δslot` per
direction (Q64.64 fixed-point), **not** V3 ticks/geometric means (irrelevant to a
full-range CPMM) and **not** the ratio-of-time-weighted-reserves form (valid but
non-standard — a consumer would be surprised). The fixed window is the correct
risk primitive: a since-open window over-smooths old positions (a real sustained
decline never moves the mark → underwater positions can't be liquidated → bad
debt) and under-protects fresh ones. A constant lookback is responsive and
protective at every position age.

Q64.64: `price_q64(out,in) = (out << 64) / in`. `(u64 << 64)` maxes at
`2^128 − 2^64` (fits u128); 64 fractional bits keep precision on a <1 SOL/token
price, 64 integer bits cover any sane price. Both directions stored so the oracle
is general (you can't invert a time-averaged ratio).

## Contract (state)

`Pool` gains a live cumulative head plus a ring of periodic snapshots. The two
cumulatives are **wrapping** Q64.64 price integrals (mod 2^128; a consumer
recovers a window with `wrapping_sub`, exact while the window sum < 2^128 — every
realistic window). Wrapping avoids a checked-add that eventually fails on a
long-lived pool.

```
// live head — advanced on EVERY swap
cum_sol_per_tok: u128   // Σ price_q64(sol_reserve, token_reserve) × Δslot
cum_tok_per_sol: u128   // Σ price_q64(token_reserve, sol_reserve) × Δslot
last_cum_slot:   u64
// history — snapshot written when spacing elapsed
observations:    [Observation; TWAP_RING_SIZE]   // {slot:u64, cum_sol_per_tok:u128, cum_tok_per_sol:u128}
obs_head:        u16
```

`Observation` = `{ slot, cum_sol_per_tok, cum_tok_per_sol }` (40 bytes). Ring = 16
→ Pool grows ~682 bytes; `Pool::LEN` bumped, **and `Pool` is `Box`ed in all four
contexts** (swap/create/add/remove) or the `try_accounts` stack frame overflows
4096 B. **Layout change ⇒ existing pools incompatible; pre-mainnet, recreate.**

## Update (per swap)

In `swap::handler`, immediately after the pre-swap `sol_reserve` / `token_reserve`
are read (swap.rs:77-78) and the empty-pool check, before `handle_buy`/`sell`:

1. **Advance both heads** from `last_cum_slot` using the *pre-swap* reserves (the
   prices that held until now): `accumulate_price(cum, price_q64(out,in), Δslot)`
   for each direction (wrapping); `last_cum_slot = now`.
2. **Snapshot to ring** only if `now − newest_obs.slot ≥ MIN_OBS_SPACING_SLOTS`.
   The head captures *every* swap; the ring spans `~TWAP_RING_SIZE × spacing` slots
   = the **max** lookback a reader can request, independent of swap frequency.

Gate: if `sol_reserve < MIN_SPOT_RESERVE` (dust-pool floor, mirrors torch
`MIN_SPOT_POOL_SOL`), advance the clock (`last_cum_slot = now`) but **don't**
accumulate — a thin pool's price isn't trustworthy, and advancing the clock
avoids mis-weighting the gap at the next above-floor swap.

## Read API (deep_pool owns the math; torch calls it, no CPI)

```
Pool::read_twap_sol_per_tok(sol_reserve_now, token_reserve_now, now_slot, lookback_slots)
    -> Option<u128>   // Q64.64 time-weighted SOL-per-token price
```

**The caller picks the window** (`lookback_slots`) — window length is *policy*, so
it belongs to the consumer, not the shared primitive; the ring/spacing are just
storage + max lookback. Lazily extends the head to `now` using current reserves
(`cum_now = accumulate_price(cum_sol_per_tok, price_q64(sol_now, tok_now), now − last_cum_slot)`
— valid because no swap since `last_cum_slot` ⇒ price unchanged), then anchors the
window at the **newest observation at least `lookback_slots` old** (largest slot ≤
`now − lookback`), `wrapping_sub`s, and divides by elapsed slots → the
time-weighted Q64.64 price. The realized window is ≥ `lookback` (≤ `lookback +
spacing`). `None` if the ring holds no observation that old — the pool is younger
than the requested window → consumers fail closed. The reverse direction lives in
`cum_tok_per_sol` if a caller needs it (no reader exposed yet — YAGNI).

**No read-side depth floor (by design).** The read deliberately extrapolates the
*current* price over the gap even when the pool is sub-floor — a position underwater
at a held crashed mark MUST stay liquidatable (bad debt can't sit stranded behind a
depth gate; see torch's `liquidation_proceeds_when_pool_thin`). A read-side
`MIN_SPOT_RESERVE` gate was considered in the v7 re-audit and **rejected** for this
reason. The write-side gate (skip sub-floor accumulation) keeps the ring *history*
clean; the read then extends from that clean history. Integrators that want a
stricter policy apply their own liquidity floor (audit I-8).

torch reads it from the `deep_pool` + `deep_pool_token_vault` accounts already in
its liquidation contexts — no new CPI, no new accounts. **Note:** unlike the
reserve-ratio sketch, this *does* change torch's pricing math — `twap_value_in_sol`
/ `twap_tokens_to_seize` now take a Q64.64 price (see torch-side changes).

## Ratchet: DROPPED

torch's per-observation ratchet (`clamp_observation_sol`,
`observation_out_of_band`, `MAX_PER_OBS_DEVIATION_BPS`) is removed. A true fixed
window already dilutes a single out-of-band swap by its dwell-time in the window —
the per-step clamp was compensating for coarse sampling that no longer exists.
`torch_sim.py` was updated to match in V21 (ratchet/clamp dropped; the sim mirrors
the keeperless price-cumulative oracle), so sim and on-chain agree again.

## Math ownership

- **In deep_pool (done):** `math::price_q64`, `math::accumulate_price`,
  `Pool::record_observation` / `read_twap_sol_per_tok` / `init_oracle`, the
  `Observation` type, the ring constants. Plus their kani proofs + proptests.
- **Rewritten in torch (done, V21):** `twap_value_in_sol`, `twap_tokens_to_seize`
  now take the Q64.64 price: `value = amount × price >> 64`;
  `tokens = (grossed_debt << 64) / price`. Their kani proofs updated accordingly.
- **Delete entirely:** `observation_out_of_band`, `clamp_observation_sol`,
  `advance_cumulative` (torch's), `record_observation_into`, torch's
  `record_observation` crank + `RecordObservation` context/ix,
  `Treasury.twap_observations`/`twap_head`.

## torch-side changes (done in V21)

All shipped in the V21 closed-loop-leverage migration:

1. Dropped `twap_observations` + `twap_head` from `Treasury` (state.rs); LEN shrank.
2. Deleted `record_observation_into` + its 5 writers (opens/closes) and the crank.
3. Rewrote `twap_value_in_sol`/`twap_tokens_to_seize` to the Q64.64 price + proofs.
4. `read_twap_mark` in liquidate_long/short → deserialize the deep_pool `Pool` and
   call `Pool::read_twap_sol_per_tok(.., LIQ_TWAP_LOOKBACK_SLOTS)` (pool accounts
   already in those contexts); feed the returned price to the rewritten pricing
   fns. **torch owns the window** — add a torch-side `LIQ_TWAP_LOOKBACK_SLOTS`
   constant (pick for *your* liquidation risk; ~53 min full-ring is conservative,
   ~10–20 min is snappier). The `twap_ltv` trigger / seize-clamp *flow* is
   unchanged; only the price source + units change.
5. `torch_sim.py`: drop the ratchet to keep the sim authoritative.

## Constants (deep_pool)

`TWAP_RING_SIZE = 16`, `MIN_OBS_SPACING_SLOTS = 500`, `MIN_SPOT_RESERVE` (SOL
floor). Drop `MAX_PER_OBS_DEVIATION_BPS`. Window ≈ 16×500 = 8000 slots (~53 min @
400ms). Tunable.

## Security / manipulation

Resistance = window length: moving the mark requires holding an off-price across
the lookback, bleeding to arbitrage every block. `MIN_SPOT_RESERVE` gates dust
pools. Fresh pool / cold ring → mark `None` → liquidation fail-closed (no
spurious trigger on a thin oracle). Add/remove-liquidity changes reserves
proportionally (price-neutral) and does **not** write observations — only swaps
move price, only swaps update the oracle.

## Tests

- **deep_pool kani (done):** `price_q64` no-panic at scale + None on zero denom +
  exact value; `accumulate_price` window-difference exact (incl. past a 2^128
  wrap). Concrete values — `price_q64` divides, so a symbolic harness explodes.
- **deep_pool proptests (done):** `price_q64` None-iff-zero-denom + monotonic in
  numerator; `accumulate_price` single-step and sequence wrapping-difference exact.
- **deep_pool litesvm (done):** warmup → mark `None`; keeperless tracking — tiny
  swaps across warped slots advance the mark with no crank, `read_twap_sol_per_tok`
  lands within 1% of spot.
- **torch (done, V21):** liquidations read the deep_pool mark; warmup → fail-closed;
  the liquidate tests are tractable (no ratchet → poke price + a few swaps across
  slots moves the mark past threshold). `liquidation_proceeds_when_pool_thin` locks
  the no-read-floor property: a held crashed price on a thin pool stays liquidatable.

## Build

Build deep_pool with **`anchor build`** — `cargo build-sbf` breaks on the litesvm
dev-dep tree (`getrandom 0.3.4` needs a custom SBF backend, then `zstd-sys` C fails
under the SBF clang). `cargo check -p deep_pool` for fast host type-checking;
`cargo test -p deep_pool --test litesvm twap` after an `anchor build`.

## Build order

design (this) → Pool state + constants + `Observation` → `price_q64` +
`accumulate_price` + `read_twap_sol_per_tok` (+ proofs) → wire into `swap::handler`
+ `create_pool` → **[done to here]** → torch consumption rewrite + deletions →
tests both sides.
