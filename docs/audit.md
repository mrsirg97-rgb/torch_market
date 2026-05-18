# V20.0.0 Audit Report

Full re-audit after the account-constraint refactor. 40 instructions, 10 split
pairs (wallet + ViaVault), shared helpers, global invariants, CPI/Token-2022,
adversarial paths.

## Summary

**No critical or high findings.** The split increased the attack surface (10
new contexts, 10 new handler paths) but every new path is account-resolved by
non-Optional constraints (no `unwrap()`), uses identical shared math/state
helpers as its sibling, and re-checks the vault-wallet link.

Five observations below — all informational. None require code changes.

## Findings

### V20-1 — MAX_WALLET cap is per-ATA, not per-controller [informational]

`quote_buy_tokens` caps `dest_balance + tokens_out ≤ MAX_WALLET_TOKENS`. For
`buy`, `dest_balance = buyer_token_account.amount`. For `buy_via_vault`,
`dest_balance = vault_token_account.amount`. This means each ATA has its own
cap. A user with multiple vaults (each requiring a separate creator pubkey)
could exceed the 1%-supply cap by accumulating into multiple vault ATAs. Same
pre-V20 evasion path (multiple wallets), no new attack surface.

`programs/torch_market/src/handlers/market.rs:106-112`

### V20-2 — Liquidation principal/interest attribution is approximate when bad debt arises in the interest-only branch [informational]

In `apply_liquidation_loan_updates`, when `actual_debt_covered ≤ accrued_interest`
AND `bad_debt > 0`, the `bad_debt` is subtracted from `borrowed_amount` and
`accrued_interest` is forcibly zeroed. This treats the entire deficit as
principal write-off even though it may include unpaid interest. `total_sol_lent`
is reduced by the same amount, keeping the on-chain accounting self-consistent,
but `total_sol_lent` slightly understates remaining principal in this edge
case. Same shape exists for shorts in `apply_short_liquidation_position_updates`.

`programs/torch_market/src/handlers/lending.rs:644-667`
`programs/torch_market/src/handlers/short.rs:676-707`

### V20-3 — `liquidation_close_bps = 10000` could strand SOL collateral on shorts [informational]

If admin sets `treasury.liquidation_close_bps = 10000`, a single liquidation
could cover 100% of debt while leaving surplus collateral (collateral required
includes the bonus). Post-liquidation state: `tokens_borrowed = 0`,
`accrued_interest = 0`, `sol_collateral > 0`. `CloseShort` requires
`short_position.tokens_borrowed > 0` so the borrower cannot retrieve the
surplus. Default value is 5000 (safe). Consider adding `close_bps < 10000`
constraint at admin-setter time if a setter is added later.

`programs/torch_market/src/contexts.rs:1612, 1670` (close_short positional constraint)
`programs/torch_market/src/constants.rs:74` (default = 5000)

### V20-4 — Vault-via-vault user attribution is per-controller-wallet [design note]

`user_position`, `user_stats`, `loan_position`, `short_position` PDAs all seed
from the signer wallet (controller), not the vault. So a vault funding two
controllers tracks volume/positions per controller, not aggregated per vault.
Rewards claims via `claim_protocol_rewards_via_vault` credit the controller's
volume but pay the SOL to the vault. This is the intended design and is
consistent with `link_wallet` semantics.

### V20-5 — Token-2022 fee leakage on short round-trips is self-correcting [informational]

Each open_short/close_short round trip causes the treasury_lock token account
to lose `tokens × fee_bps/10000` to the Token-2022 withhold authority. Because
`check_short_caps` uses the *current* `treasury_lock_token_account.amount` as
the cap base (not a tracked figure), future borrow limits naturally tighten as
fees accumulate. Harvested fees are eventually swapped back to SOL via
`harvest_fees` + `swap_fees_to_sol`. No state-vs-runtime drift.

`programs/torch_market/src/handlers/short.rs:64-102`

## Verified properties (no findings)

### Per-context constraints (40 ixs)
- Every `*ViaVault` context has the mandatory triple
  `torch_vault` + `vault_wallet_link` + `vault_token_account` (no Optional, no
  `as_ref().unwrap()`).
- `vault_wallet_link.vault == torch_vault.key()` cross-check present on all 10
  via_vault contexts.
- `vault_wallet_link` PDA seeded by signer wallet, preventing cross-controller
  attacks.
- D-2 defense-in-depth (`bonding_curve.migrated`, `!bonding_curve.reclaimed`)
  present on `Liquidate`, `CloseShort`, `LiquidateShort`, `Borrow`, `OpenShort`
  (both wallet + via_vault variants).
- D-3 arg validation (`MIN_*` checks, zero-amount rejection) present on
  `BuyArgs`, `SellArgs`, `BorrowArgs`, `OpenShortArgs`, `CloseShort.token_amount`.

### Helper consistency
- Every split pair calls identical shared helpers
  (`compute_*`, `quote_*`, `finalize_*`, `apply_*`) with identical args
  sourced from corresponding accounts.
- Only differences between wallet/via_vault are:
  1. SOL funding (System.transfer vs direct-lamport-shift);
  2. Token-destination ATA (signer ATA vs vault ATA);
  3. Vault SOL accounting (`sol_balance`, `total_spent`, `total_received`).
- No diverging math or state-update branches.

### Global invariants
- `treasury.sol_balance` state always matches actual treasury PDA lamport
  flows (minus `star_sol_balance` carve-out).
- `treasury.total_sol_lent` increases on borrow, decreases on repay
  (`principal_paid`) and liquidation (`remaining_paid + bad_debt`).
- `treasury.total_collateral_locked` increases on borrow `net_deposited`,
  decreases on full repay `collateral_returned` and liquidation
  `actual_collateral_seized`.
- `treasury.short_collateral_reserved` mirrors all
  `sum(short_positions.sol_collateral)` mutations.
- Vault: `sol_balance` debited before spend, credited on receipt, with
  `total_spent`/`total_received` cumulative counters.

### Interest accrual
- `apply_interest_accrual` and `apply_short_interest_accrual` advance
  `last_update_slot` on every path (zero-debt, zero-elapsed, normal) —
  Kani-proven (harnesses 71, 73).
- No stale-slot bug on re-borrow of a fully-repaid-but-not-closed position.

### Bad-debt formula
- Algebraic identity:
  `bad_debt = debt_to_cover - actual_debt_covered` holds for both lending and
  shorts.
- Sum-conservation:
  `actual_debt_covered + bad_debt + (total_debt - debt_to_cover) = total_debt`.

### CPI surface
- All `CpiContext::new_with_signer` sites use the correct PDA seeds for the
  signing authority (bonding_curve, treasury, treasury_lock, torch_vault).
- No re-entrancy possible: all CPIs go to Token-2022, System, DeepPool,
  Associated Token Program — none of which call back into torch_market.
- Token-2022 fee handling uses reload-and-diff on borrow's collateral inflow
  (lending.rs:215-238). Other paths accept fee leakage as expected, with
  `harvest_fees` reclaiming via DeepPool swap.

### Adversarial split-specific
- Anchor discriminators prevent wallet ix from accepting via_vault accounts
  (and vice-versa).
- Mix-and-match attacks (vault A's torch_vault + vault B's link) blocked by
  `vault_wallet_link.vault == torch_vault.key()`.
- Cross-controller link attacks blocked by `vault_wallet_link` PDA seed
  (always seeded by the signer wallet).
- Link/unlink race: unlinked vault_wallet_link account is unresolvable;
  re-linked link points to new vault, old vault rejected by cross-check.
- Vault can't be drained beyond `vault.sol_balance` (require! before every
  spend).
- Borrow with `collateral = 0` rejected by `calc_ltv_bps` division-by-zero
  guard.

## Verdict

V20.0.0 is safe to deploy. The account-constraint refactor materially reduced
the attack surface compared to V19 (removed Optional `unwrap()` panic risk,
moved arg validation to account-resolution time, added defense-in-depth on
post-migration ops) while introducing no new exploitable paths. The five
informational findings are accounting nuances or admin-misconfiguration edge
cases, not exploits.
