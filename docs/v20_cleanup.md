# v20 cleanup — tier A

Pure hygiene. No behavior changes. Each item is verified against the current tree.

## 1. delete `validate_deep_pool`

Defined in `pool_validation.rs:52`, called nowhere. Every context that accepts the deep_pool account already enforces `#[account(address = derive_deep_pool(&derive_torch_config(), &mint.key()) @ ...)]`, which proves both identity and shape (the address can only exist if deep_pool created it). The byte-slice owner check in `validate_deep_pool` adds nothing the PDA derivation doesn't already give us.

**Contract:** none — function has zero callers.
**Touches:** `pool_validation.rs` only.

## 2. delete `require_price_in_band` + `MAX_PRICE_DEVIATION_BPS`

Defined in `pool_validation.rs:99` and `constants.rs:92`, called nowhere in handlers. `docs/verification.md:184` says it's "retained for `swap_fees_to_sol` ratio gating," but `swap_fees_to_sol` (handlers/treasury.rs:87–104) uses its own inline asymmetric gate (`current_ratio >= baseline * 1.2`) and never calls `require_price_in_band`.

Wiring it in would be wrong both places:
- In `swap_fees_to_sol`: the inline gate is strictly more restrictive on the up side. The lower bound (`>= 0.5x`) is never reached given the `>= 1.2x` gate. The upper bound (`<= 1.5x`) would block sells exactly when the treasury should be selling. No-op or actively harmful.
- In margin paths: contradicts the depth-anchored model in `docs/risk.md` ("no oracles, keepers, or governance required"). The whole V7 thesis is that manipulation cost replaces baseline-deviation gating.

**Contract:** none — constant and function have zero callers.
**Touches:** `pool_validation.rs`, `constants.rs`, `kani_proofs.rs` (proofs #59 / #60 / #61 and the local `calc_price_in_band` mirror at lines 1773–1879 all reference the removed constant; delete the section), `docs/verification.md` (remove line 184 reference and the `MAX_PRICE_DEVIATION_BPS` row in the constants table; update proof count 75 → 72).

`RATIO_PRECISION` stays — `swap_fees_to_sol`'s inline gate uses it.

## 3. static asserts on buy-split rate invariant

`compute_buy_split` (handlers/market.rs:67–70) computes `sol_to_treasury_split = total_split - creator_sol` via `checked_sub`. This only succeeds for all reachable inputs because `creator_rate_bps(x) <= treasury_rate_bps(x)` holds across `x ∈ [0, target]`. Both functions are linear and clamped, so the invariant reduces to two endpoint checks:

- `CREATOR_SOL_MIN_BPS <= TREASURY_SOL_MAX_BPS` (at `x = 0`)
- `CREATOR_SOL_MAX_BPS <= TREASURY_SOL_MIN_BPS` (at `x = target`)

Today: `20 <= 1750` ✓ and `100 <= 250` ✓. A future contributor bumping any of the four constants could silently break buys at one end of the curve with a `MathOverflow` rather than a compile failure.

**Contract:** add `const _: () = assert!(...)` for both endpoint relations in `constants.rs`. No runtime cost, no IDL impact.
**Touches:** `constants.rs` only.

## 4. unify `claim_protocol_rewards` lamport ops

`claim_protocol_rewards` (handlers/protocol_treasury.rs:139–148) uses raw `-=` / `+=` on lamports; `claim_protocol_rewards_via_vault` (lines 169–176) uses `checked_sub` / `checked_add` with `MathOverflow`. Same operation, different style. Raw `-=` panics on underflow, which terminates the program — checked variants surface as a typed Anchor error.

**Contract:** signature unchanged. Both paths return the same `MathOverflow` on the (unreachable today) underflow case.
**Touches:** `handlers/protocol_treasury.rs` only.

---

## order of work

Sequential, each commit independent and reviewable:

1. (#1) `validate_deep_pool` delete — zero risk
2. (#3) buy-split static asserts — zero risk, catches future regressions
3. (#4) claim_protocol_rewards unify — zero risk
4. (#2) `require_price_in_band` + `MAX_PRICE_DEVIATION_BPS` + kani section + verification.md — touches the most surface, do last

## verification

- `anchor build` after each commit
- `cargo kani` after #4 — expect 72/72 (was 75/75; three circuit-breaker proofs deleted)
- `cargo test` after #4 — proptests unaffected (math layer untouched)

Tier B items (liquidation pool-thin blocker, never-close loan/short PDAs, `saturating_sub` → `checked_sub` on aggregate counters) follow below.

---

# v20 cleanup — tier B

Behavior changes. Each landed with a flipped pin-test that asserts the new behavior.

## 5. drop pool-thin gate from liquidate paths

**Problem.** `liquidate`, `liquidate_via_vault`, `liquidate_short`, `liquidate_short_via_vault` all called `require_min_pool_liquidity(pool_sol)?` before doing any work. If a pool drained below `MIN_POOL_SOL_LENDING = 5 SOL` after a position was opened, the position became **un-liquidatable**. Borrower keeps SOL with worthless collateral, treasury eats indefinite bad debt.

**Fix.** Removed the depth check from the four liquidate handlers. Kept the `pool_sol > 0` divide-by-zero guard. New positions still gated by the depth-tier LTV (`get_depth_max_ltv_bps(pool_sol) == 0` → `PoolTooThin` in `check_borrow_ltv` / `check_short_ltv`), so depth still prevents *opening* in thin pools — it just no longer strands *existing* ones.

The `require_min_pool_liquidity` function and its sole call sites are gone. The `MIN_POOL_SOL_LENDING` constant stays (used by `get_depth_max_ltv_bps`).

**Touches:** `handlers/lending.rs`, `handlers/short.rs`, `pool_validation.rs` (function deleted), `tests/litesvm/tier_b.rs` (flipped), `tests/litesvm/short.rs` (flipped to assert `ShortNotLiquidatable` — the path past the depth gate hits the LTV check, which says the position is healthy at the thin-pool price), `tests/litesvm/lending.rs` (added `borrow_pool_too_thin_blocks_new_position` to keep `PoolTooThin` coverage on the open path).

## 6. close loan/short PDAs on full settlement

**Problem.** `repay` / `liquidate` / `close_short` / `liquidate_short` zeroed `borrowed_amount`, `accrued_interest`, `collateral_amount` on full settlement but never closed the PDA. Rent stayed locked until manual recovery (no instruction existed for this). At ~0.002 SOL per stranded slot, this is a small per-token leak that compounds across users.

**Fix.** Added `close_account_to(account, destination)` helper in `handlers/lending.rs` (`pub(crate)` so `short.rs` imports it). It refunds lamports to `destination`, zeroes the data, reassigns to the system program, and reallocs to 0 — making the slot available for re-init via Anchor's `init_if_needed`.

Wired into the six settlement paths:
- `repay` + `repay_via_vault` → close `loan_position` to `borrower` when `is_full_repay`
- `liquidate` + `liquidate_via_vault` → close `loan_position` to `borrower` when `fully_liquidated`
- `close_short` + `close_short_via_vault` → close `short_position` to `shorter` when `is_full_close`
- `liquidate_short` + `liquidate_short_via_vault` → close `short_position` to `borrower` when `fully_liquidated`

Destinations match the rent payers (Anchor `payer = borrower` / `payer = shorter` at init), so rent is correctly refunded.

**Touches:** `handlers/lending.rs`, `handlers/short.rs`, `tests/litesvm/tier_b.rs` (flipped both PDA-leak tests to assert closure), `tests/litesvm/lending.rs::repay_full_returns_collateral` (flipped), `tests/litesvm/short.rs::close_short_full` (flipped).

## 7. saturating_sub → checked_sub on aggregate counters and liquidation math

**Problem.** Aggregate counters (`treasury.total_sol_lent`, `total_collateral_locked`, `active_loans`, `short_collateral_reserved`, `sol_balance` on seizure; `short_config.total_tokens_lent`, `active_positions`) plus the per-position liquidation math used `saturating_sub`. By construction these subtractions are invariant-safe, but `saturating_sub` silently clips on underflow — masking any future bug that violates an invariant.

Per AGENTS.md "errors include context, never silent": prefer failing loud.

**Fix.** Converted every `saturating_sub` in `handlers/lending.rs` and `handlers/short.rs` to `checked_sub(...).ok_or(MathOverflow)?`. Includes:
- Aggregate counter decrements on `repay`, `liquidate`, and their `_via_vault` variants
- Same for `close_short`, `liquidate_short`, and their `_via_vault` variants
- `available_sol` calculation in `check_borrow_caps` (was `treasury.sol_balance.saturating_sub(short_reserved)`)
- `bad_debt` computation in both `compute_liquidation` and `compute_short_liquidation`
- All position-field decrements inside `apply_liquidation_loan_updates` and `apply_short_liquidation_position_updates`

**Contract:** if all invariants hold (proven by kani + proptest on the math layer), no `checked_sub` ever returns `None`. If one ever does, the tx fails with `MathOverflow` and the bug is visible — instead of the protocol silently running on drifted state.

**Touches:** `handlers/lending.rs`, `handlers/short.rs`. No test updates needed — invariant-safe paths still pass; if any future change breaks an invariant, the corresponding test fails loudly.

---

## verification (tier B)

- `cargo build-sbf` clean
- `cargo test --test litesvm` → 100/100 tests pass in ~6s
- Coverage gate (`coverage::every_reachable_variant_has_a_test`) still green — `PoolTooThin` moved from liquidate paths to the new borrow-path test, no other variants regressed
- Known parallel-test flake: occasional `ComputationalBudgetExceeded` on `create_token` under heavy parallel scheduling. Not a Tier B regression — pre-existing CU sensitivity. Tracked separately.

---

# v20 cleanup — tier B+ (dead-code removal)

IDL-breaking. Bundle with the SDK sync that already needs to drop the `paused` field.

## 8. delete `enable_short_selling` instruction

**Problem.** The `enable_short_selling` ix exists but is **unreachable** through any valid flow. The context constraint requires `!treasury.short_selling_enabled`, but `create_token` sets `short_selling_enabled = true` at mint creation. Chicken-and-egg makes the ix permanently un-callable. The handler, context, error variant, and event are pure scaffolding from an earlier design where shorts started disabled.

**Fix.** Deleted:
- `enable_short_selling` dispatcher entry in `lib.rs`
- `enable_short_selling` handler in `handlers/short.rs`
- `EnableShortSelling` context in `contexts.rs`
- `ShortAlreadyEnabled` error variant in `errors.rs`
- `ShortSellingEnabled` event in `handlers/short.rs`
- The `ShortAlreadyEnabled` exemption from `tests/litesvm/coverage.rs`

`ShortConfig` PDA is still initializable — `OpenShort` and `OpenShortViaVault` contexts already do `init_if_needed` on it, so the first short on a mint creates it.

**Touches:** `lib.rs`, `handlers/short.rs`, `contexts.rs`, `errors.rs`, `tests/litesvm/coverage.rs`, `docs/architecture.md`.

**IDL impact:** instruction count drops by 1, one error variant removed (codes after `ShortAlreadyEnabled` shift down by 1), one event removed. SDK must drop `buildEnableShortSellingTransaction` + `EnableShortSellingParams` type + index exports.

## 9. dead error variant sweep

**Problem.** 12 error variants in `errors.rs` are never raised by any handler or context constraint. They were either removed during refactors (RatioAboveThreshold, AtReserveFloor, BuybackTooSoon — the buyback handler now returns `Ok(())` silently instead), absorbed by clamping logic (RepayExceedsDebt — handler clamps payment), handled by Anchor's built-in constraints (ProtocolTreasuryAlreadyInitialized — `init` constraint handles re-init), or were defensive scaffolding (InsufficientUserBalance, WalletNotLinked, InvalidTokenAccount, InsufficientTreasuryTokens) that no constraint actually invokes. They bloat the IDL and confuse readers chasing imaginary error paths.

**Fix.** Deleted 12 variants from `errors.rs`:
- `RepayExceedsDebt`, `BuybackTooSoon`, `RatioAboveThreshold`, `AtReserveFloor`, `PoolCreationFailed` — replaced by silent `Ok(())` or clamping
- `ProtocolTreasuryAlreadyInitialized`, `ProtocolTreasuryBelowFloor` — handled upstream by Anchor `init` / reserve_floor=0
- `InsufficientUserBalance`, `WalletNotLinked`, `InvalidTokenAccount`, `InsufficientTreasuryTokens` — unused scaffolding
- `PriceDeviationTooHigh` — was raised by the deleted `require_price_in_band`

**Touches:** `errors.rs`, `tests/litesvm/coverage.rs` (trim EXEMPT list).

**IDL impact:** 12 error variant codes removed; every variant after the lowest deleted one shifts down. Combined with #8 + the earlier `ProtocolPaused` removal, this is the cumulative re-numbering the SDK must re-import.

## 10. internal dead-branch removal

**Problem.** Three handler branches were unreachable given upstream constraints:
1. `swap_fees_to_sol` had an `else { token_amount }` for `baseline_initialized = false`, but the SwapFeesToSol context constraint requires `baseline_initialized = true` (always set by `migrate_to_dex`). The else was dead.
2. `claim_protocol_rewards` wrapped its lamport movements in `if claim_amount > 0`, but `compute_claim` already enforces `claim_amount >= MIN_CLAIM_AMOUNT (0.1 SOL)`. Always true.
3. Same `if claim_amount > 0` in `claim_protocol_rewards_via_vault`.

**Fix.** Removed all three dead branches; reduced indentation, added explanatory comments.

**Touches:** `handlers/treasury.rs`, `handlers/protocol_treasury.rs`. No IDL impact.

## 11. defense-in-depth constraint adds

**Problem.** Three places relied on transitive invariants instead of explicit constraints:
1. `Sell` context only checked `!bonding_complete`. Reclaimed tokens had `real_sol_reserves = 0`, so sells failed downstream with `InsufficientSol` from `compute_sell`. Reader had to derive the reclaim guard transitively.
2. `payer_lp_account` and `deep_pool_lp_account` in `MigrateToDex` were `/// CHECK: AccountInfo` with no constraint. Deep_pool's `create_pool` validated them inside the CPI, so a malformed account surfaced as a deep_pool error rather than a torch_market constraint failure.
3. `read_token_account_balance` parsed bytes at offset 64..72 without checking the account was actually a Token-2022 account. Callsites were safe in practice but the function silently produces nonsense on a wrong account.

**Fix.**
1. Added `constraint = !bonding_curve.reclaimed @ AlreadyReclaimed` to `Sell` context. Updated the `sell_after_reclaim_rejected` test to assert the new error.
2. Added `address = get_associated_token_address_2022(...)` constraints to both LP-ATA `AccountInfo`s in `MigrateToDex`. Wrong account now fails at constraint time with `InvalidPoolAccount`.
3. Added an `account.owner == &TOKEN_2022_PROGRAM_ID` check at the top of `read_token_account_balance`. Returns `InvalidPoolAccount` on mismatch.

**Touches:** `contexts.rs`, `pool_validation.rs`, `tests/litesvm/sell.rs` (test renamed + assertion updated), `tests/litesvm/coverage.rs` (moved `InsufficientSol` to EXEMPT since the `!reclaimed` constraint replaces it on the assertion path).

**IDL impact:** none — adding constraints doesn't change instruction signatures.
