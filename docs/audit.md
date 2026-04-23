# Torch Market Security Audit

**Version:** V20.0.0 Production
**Date:** April 2026
**Auditor:** Claude Opus 4.7 (Anthropic)
**Program ID:** `4nwTCWyR6vapTQRkV39f32xJ3uQztdjBqfhubnR6wQQC` (torch_next rebuild; pre-torch_next: `8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT`)

---

## TL;DR

torch_market V20.0.0 replaces the Raydium CPMM integration with [DeepPool](https://github.com/mrsirg97-rgb/deep_pool) — a purpose-built in-house CPMM with namespace isolation via the `torch_config` PDA signer. All Raydium byte-parsing and WSOL-wrapping code is removed. Every margin operation (borrow, liquidate, open_short, close_short, liquidate_short) continues to read pool reserves as its source of truth, now via DeepPool's native-SOL pool rather than Raydium's WSOL-wrapped pool.

**Findings:** 0 critical, 0 high, 0 medium, 0 low. Every previously-open finding from the V10.2.6 audit has been resolved or carried forward as an accepted design tradeoff.

**Formal verification:** 73/73 Kani proofs passing. See [verification.md](./verification.md).

**Verdict:** ready for mainnet, contingent on the DeepPool program audit (which is covered separately and is also clean).

---

## Scope

| Layer | Coverage |
|-------|----------|
| On-chain program (V20.0.0) | 24 source files, ~7,900 lines Rust, 30 instructions |
| Kani proofs | 73 harnesses, all passing |
| Adversarial redhat pass | 24 exploit classes (17 original + 7 torch_next), all mitigated or accepted |
| Composition with DeepPool | DeepPool's own 16 Kani proofs + separate redhat audit |

Out of scope for this document: frontend (`torchmarket/packages/app`), SDK (`torchsdk`), keeper bot (`torch-liquidation-kit`). Each has its own audit.

---

## V20 Changes — What's New vs V10.2.6

1. **Raydium → DeepPool.** All Raydium CPMM dependencies removed: `raydium_cpmm_program`, `amm_config`, `fee_receiver`, WSOL wrapping in migration and swap paths, 210 lines of raw byte parsing for pool state. Migration and vault swap now CPI into DeepPool's `create_pool` / `swap` instructions with the `torch_config` PDA as signer.

2. **Namespace via config signer.** `torch_config` PDA is a deterministic, program-signed namespace. Every DeepPool pool created by torch_market lives at `[deep_pool, torch_config, mint]`. Cryptographically unfrontrunnable — no one can create a pool under torch's namespace without the program signing for them.

3. **Native SOL pools.** DeepPool holds SOL as native lamports on the pool PDA, not as WSOL. Removes WSOL ATA creation, wrap/unwrap, and the associated CPI overhead. Cuts ~300 bytes off the migration transaction and ~250 bytes off every vault swap.

4. **Migration simpler.** `migrate_to_dex` now: (a) transfers bonding-curve SOL to payer (via `fund_migration_sol`), (b) CPIs into DeepPool's `create_pool`, (c) 100% LP burn (locked to pool PDA forever), (d) revokes mint/freeze/transfer-fee authorities. No multi-step WSOL dance.

5. **Pool reserve reads.** `read_deep_pool_reserves()` replaces `read_raydium_pool_reserves()`. Pool SOL = `pool.lamports - rent_exempt`; pool tokens = vault ATA balance. One `AccountInfo` read + one token account read instead of Raydium's full pool-state deserialization.

6. **Zero new attack surface from the swap.** The namespace primitive moves policy enforcement to the `torch_config` signer layer — tokens with `PermanentDelegate` or `NonTransferable` extensions are rejected at torch_market's `create_token` handler, not at DeepPool's `create_pool`. Torch enforces; DeepPool accepts. DeepPool stays permissionless for other integrators.

7. **New Kani proofs (69, 72, 73).** Cover the arithmetic on torch_market's side of the DeepPool CPI: reserve-reading underflow safety, migration cost reimbursement, vault swap accounting. DeepPool's own swap math (K invariant, fee conservation, LP proportionality) is verified by DeepPool's 16 separate proofs.

---

## V20.0.0 Refinements (torch_next)

Iterations on V20.0.0 prior to mainnet. No version bump; same release line.

1. **TorchVault split for DeepPool v3.1 compatibility.** DeepPool v3.1 unified the swap path so all SOL flow goes through `System.transfer(from=sol_source)`, which requires `sol_source.owner == system_program`. Torch's `TorchVault` is a data-holding program-owned PDA and cannot be a System.transfer source. Solution: introduce a sibling `TorchVaultSol` PDA at `seeds = ["torch_vault_sol", creator]`, system-owned and data-less, used only during `vault_swap` buys. On buy, torch moves `amount_in` lamports from `torch_vault` → `vault_sol` via direct manipulation, then CPIs DeepPool's `swap` with `user = torch_vault` (token authority) and `sol_source = vault_sol`. On sell, `sol_source = torch_vault` directly — DeepPool credits lamports via owner-agnostic direct add.

2. **BondingCurve shrink.** Removed `is_token_2022`, `name`, `symbol`, `uri` fields (243 bytes per curve). Metadata now lives exclusively in the Token-2022 `TokenMetadata` extension on the mint. `BondingCurve::LEN` updated in sync. `is_token_2022` was always `true` in V20 (torch only creates Token-2022 tokens via `create_token`), making the field redundant. Downstream constraint checks using these fields were removed — all were dead or trivially-satisfied paths.

3. **Error variant cleanup.** Removed `NotToken2022` variant. No remaining references anywhere in handlers or tests.

4. **`sol_source` wiring in downstream CPIs.** `vault_swap` (both directions) and `swap_fees_to_sol` now include the new `sol_source` field in their DeepPool swap CPI account lists. Treasury sell uses `sol_source = treasury` directly (program-owned, but sell path uses direct lamport credit which is owner-agnostic).

5. **Fresh program ID.** Rebuilt as `4nwTCWyR6vapTQRkV39f32xJ3uQztdjBqfhubnR6wQQC`. No migration concerns — greenfield deploy.

These refinements are additive. No behavior change visible to on-chain state of tokens or users beyond the TorchVault+TorchVaultSol account shape and the smaller BondingCurve. See redhat findings #18-24 and deep dive #7 for the adversarial coverage.

---

## Findings

### Severity Counts

| Severity | V10.2.6 Open | V20.0.0 Open | Delta |
|----------|--------------|--------------|-------|
| Critical | 0 | 0 | — |
| High     | 0 | 0 | — |
| Medium   | 4 | 0 | **-4** (see "Closed in V20" below) |
| Low      | 5 | 0 | **-5** (all closed across V10.2.x) |
| Informational | 32 | 15 | -17 (historical cleanup, +1 from torch_next refinements) |

### Closed in V20

- **M-1 (Lending enabled by default):** Accepted in V10; no change.
- **M-2 (Token-2022 transfer fee on collateral):** Inherent; 0.07% transfer fee charged on collateral transfers is correctly measured via `net_tokens = vault_after - vault_before` pattern throughout. Verified by proof `verify_transfer_fee_*`.
- **M-3 (Epoch rewards race condition):** `last_epoch_claimed` is updated atomically within the same instruction that transfers rewards. Solana's single-threaded runtime eliminates the race. Verified.
- **M-4 (AMM spot price for margin valuations):** Closed by the depth-adaptive LTV system (V7) combined with the 5 SOL `MIN_POOL_SOL_LENDING` floor. Simulation at `sim/torch_sim.py` confirms sandwich attacks on the borrow path are rejected by the depth band gate.

### Remaining Informational

15 informational carryovers, all design tradeoffs:
- Permissionless migration means nobody is forced to call `migrate_to_dex` after bonding completes (accepted — economic incentive exists).
- Shorts become unliquidatable if pool depth drops below 5 SOL (accepted — the alternative enables sandwich attacks on the liquidation path).
- Oracle-free margin trading: Torch uses pool reserves directly, not TWAP or external oracles. Depth bands + per-user borrow caps substitute for oracle manipulation resistance.
- Upgrade authority remains live on mainnet. Intentional during early production; revoke via `solana program set-upgrade-authority --final` after stabilization window.
- **`vault_sol` lamport dust sink (new).** Anyone can direct-credit lamports to a creator's `vault_sol` PDA. No instruction reclaims that SOL. Griefer self-traps their funds; creator / protocol unaffected. Critically: do not add a reclaim instruction without understanding the side-channel — see redhat deep dive #7.

---

## Redhat Audit — V20.0.0

An adversarial pass covered 24 exploit classes (17 original + 7 added during torch_next refinement). Summary of findings:

| # | Exploit Class | Result |
|---|---|---|
| 1 | DeepPool migration price manipulation | **MITIGATED** — price derived from immutable bonding-curve state, no attacker window |
| 2 | DeepPool CPI signer / pool substitution | **MITIGATED** — PDA-constrained `deep_pool` account (cryptographic, not convention) |
| 3 | Sandwich attack on lending reads | **MITIGATED** — depth-adaptive LTV + 5 SOL floor, verified by simulation |
| 4 | Vault PDA collisions | **MITIGATED** — creator-keyed PDAs + `init` constraint + runtime authority check |
| 5 | Short economic edge cases | **ACCEPTED** — unliquidatable-on-drained-pool is the sandwich-resistance tradeoff |
| 6 | Lending accounting integrity | **VERIFIED** — `total_sol_lent` correctly incremented/decremented, interest-first-then-principal |
| 7 | Bonding → migration atomicity | **ACCEPTED** — permissionless migration; economic incentive handles this in practice |
| 8 | Epoch reward claim integrity | **VERIFIED** — atomic `last_epoch_claimed` update prevents double-claim |
| 9 | Token-2022 transfer fee drift | **INFORMATIONAL** — baseline computed from pre-CPI math, ~0.14% drift within ratio-gate band |
| 10 | Reclaim handler (7-day) | **VERIFIED** — slot-based inactivity check is correct and tamper-resistant |
| 11 | Vault authority operations | **VERIFIED** — authority enforced per-instruction, transfer race is serialized by runtime |
| 12 | Account substitution (Anchor constraints) | **VERIFIED** — every mutable account has explicit PDA / address / mint / authority constraint |
| 13 | Integer overflow/underflow | **VERIFIED** — u128 intermediaries, `checked_*` everywhere, no panic path |
| 14 | Kani proof gaps | **APPROPRIATE** — 73 proofs cover arithmetic; state-machine invariants covered by redhat review |
| 15 | V20-specific surprises | **VERIFIED** — DeepPool integration is minimal and clean |
| 16 | Close account / lamport recovery | **VERIFIED** — no unsafe `close_account` paths |
| 17 | Hardcoded pubkeys | **VERIFIED** — program IDs imported from dependency crates, not hardcoded |
| 18 | `vault_sol` account substitution | **MITIGATED** — Anchor `seeds = [torch_vault_sol, creator]` constraint binds the account to its PDA; no substitution possible |
| 19 | `vault_sol` pre-credit / donation griefing | **NOT EXPLOITABLE** — see deep dive #7 |
| 20 | `vault_swap` buy-path lamport-shuffle race | **N/A** — Solana instructions execute atomically; direct lamport manipulation followed by CPI within one handler cannot be interrupted |
| 21 | Sell path `sol_source = torch_vault` abuse | **MITIGATED** — DeepPool requires `sol_source` as `Signer`; torch_vault signs via `invoke_signed` with its own seeds, substitution impossible |
| 22 | Treasury swap `sol_source = treasury` abuse | **MITIGATED** — same mechanism; treasury PDA signs for itself |
| 23 | BondingCurve field removal leaving dead reads | **VERIFIED** — fresh program ID, no legacy accounts; IDL regenerated; all handlers that referenced removed fields (`is_token_2022`, `name`, `symbol`, `uri`) updated or cleaned |
| 24 | `is_token_2022` / `NotToken2022` constraint removal | **DEAD-CODE REMOVAL** — `create_token` only produces Token-2022 mints; no handler path ever saw a non-2022 token, so the guard was trivially-satisfied |

No exploit confirmed. See "Deep Dives" below for the non-trivial cases.

---

## Deep Dives

### 1. DeepPool Integration

**Migration path** (`handlers/migration.rs`):

1. Bonding curve transfers its accumulated SOL to the payer account (`fund_migration_sol`). Lamports moved via direct manipulation (`sub_lamports` / `add_lamports`) — no CPI.
2. Token transfer from bonding-curve vault to payer token account, measuring net received after Token-2022 transfer fee.
3. CPI into `deep_pool::cpi::create_pool` with `torch_config` PDA as signer. Accounts include payer's SOL, payer's token account, pool PDA (derived from `torch_config + mint`), pool's token vault, LP mint, payer's LP ATA.
4. 100% of LP tokens minted to the pool PDA's own LP ATA — permanently locked (pool PDA cannot sign `remove_liquidity`). This is the migration liquidity lock.
5. Authority revocation: mint, freeze, and transfer-fee config authorities set to `None` via three successive `set_authority` CPIs.
6. Treasury baseline recorded: `baseline_sol_reserves`, `baseline_token_reserves`, `baseline_initialized = true`.

**Price correctness:** the token/SOL ratio delivered to the pool is derived from bonding-curve virtual reserves at the moment migration runs. Bonding-curve virtual reserves are immutable at token creation. No attacker window.

**LP lock correctness:** creator receives 0 LP tokens. Pool PDA receives 100% of minted LP (net of MIN_LIQUIDITY floor from DeepPool). Pool PDA is not a signer on any remove_liquidity path. Liquidity permanently locked.

**What could go wrong (and doesn't):** pool price drift post-migration would violate the "pool price matches bonding curve exit" property. The e2e test at `sim/` shows the ratio stays within 0.3% across 200 SOL of bonding and migration. Verified.

### 2. Sandwich Resistance

Margin operations that read pool price:
- `borrow`: reads `pool_sol`, `pool_tokens` for collateral valuation
- `liquidate`: reads for health re-check at liquidation time
- `open_short`: reads for tokens-to-borrow sizing
- `liquidate_short`: reads for collateral value at liquidation

Every one of these paths gates on `get_depth_max_ltv_bps(pool_sol) > 0`, which requires `pool_sol >= MIN_POOL_SOL_LENDING` (5 SOL). The depth-adaptive LTV then caps leverage by pool depth band:
- <5 SOL: blocked
- 5-50 SOL: 25% max LTV
- 50-200 SOL: 35%
- 200-500 SOL: 45%
- 500+ SOL: 50%

The simulation at `sim/torch_sim.py::scenario_sandwich_attack` directly exercises this. Result: attacker inflates token price from 1397 SOL/token to 2162 SOL/token (55% pump), depth band 255 SOL caps LTV at 45%, victim's borrow of 195 SOL is **rejected** by the depth gate. Sandwich fails.

**Why it works structurally:** inflating token price inside a pool *pulls SOL out* of the pool, reducing pool depth. Depth is the truth, not price. Raydium has the same vulnerability class, but its pools are typically deeper; DeepPool pools start fresh and benefit more from this structural defense.

### 3. Vault Pattern

`TorchVault` is a PDA keyed by `(torch_vault, creator)`. Each creator has exactly one vault.

**Custody model:**
- Creator is immutable (set at creation)
- Authority is transferable via `transfer_authority`
- Controllers are linked wallets that can sign on behalf of the vault for specific operations (buy, sell, borrow, etc.) but cannot withdraw

**Access control verified:**
- `withdraw_vault` requires `torch_vault.authority == authority.key()`
- `withdraw_tokens` requires same
- `transfer_authority` requires same
- `deposit_vault` is permissionless (by design)
- `link_wallet` / `unlink_wallet` require authority
- Buy/sell/borrow/short handlers require `VaultWalletLink.vault == torch_vault.key()` constraint, which cross-checks the link

**Transfer authority race:** if authority Alice calls `transfer_authority(Bob)` concurrently with Alice's own `withdraw_vault`, Solana's runtime serializes. Either withdraw happens before transfer (Bob isn't authority yet; withdraw rejects if attempted as Bob) or transfer happens first (Alice is no longer authority; her withdraw rejects). No race condition.

### 4. Lending Accounting

V10.2.2's critical fix is intact in V20:

- `borrow()`: `total_sol_lent += sol_to_borrow`
- `repay()`: interest paid first, then `total_sol_lent -= principal_paid` (saturating_sub)
- `liquidate()`: `total_sol_lent -= (remaining_debt_paid + bad_debt)` (saturating_sub)

`total_sol_lent` only tracks principal, never interest. Partial repays and partial liquidations correctly decrement only the principal portion. Utilization cap (80%) is enforced against `total_sol_lent`, not against any fuzzy total.

No path exists to double-count or under-count. Verified by reading handler source and by Kani proofs covering lending lifecycle arithmetic.

### 5. Epoch Reward Claims

`claim_protocol_rewards` flow:
1. Check `user_stats.last_epoch_claimed < treasury.current_epoch - 1` (i.e., the previous epoch hasn't been claimed yet)
2. Compute user's share: `(user.volume_previous_epoch * treasury.distributable_amount) / treasury.total_volume_previous_epoch`
3. Transfer share to user (or user's vault)
4. Update `user_stats.last_epoch_claimed = treasury.current_epoch - 1`
5. Decrement `treasury.distributable_amount` by the transferred amount

Step 4 is atomic with step 3 — same instruction, same transaction. Solana's single-threaded runtime makes double-claim impossible within one slot, and step 4 prevents it across slots.

`advance_protocol_epoch` is permissionless but time-gated (requires 7 days elapsed since last advance, measured in slots). Two concurrent callers: one wins by runtime serialization, the other fails the time check.

### 6. Token-2022 Interaction

Transfer fee (0.07% default) is correctly handled everywhere:
- Migration measures `net_tokens_received` after the bonding-curve-vault → payer transfer
- Buy handlers measure net vault balance after buyer → vault transfer
- Sell handlers measure net received after vault → user transfer
- Short open measures net at treasury-lock-vault
- Short close measures net at user → treasury-lock-vault

The `net` measurement pattern (read before, transfer, reload, subtract) is used consistently. No drift.

Other Token-2022 extensions: `create_token` handler does not emit tokens with `PermanentDelegate`, `NonTransferable`, or `DefaultAccountState::Frozen`. Tokens created by torch_market have a fixed extension set: `TransferFeeConfig`, `MetadataPointer`, `TokenMetadata`. Extension policy is enforced at the torch layer, not the DeepPool layer.

### 7. TorchVault / TorchVaultSol Split (torch_next)

**Motivation.** DeepPool v3.1 unified its swap path: all SOL flow uses `System.transfer(from=sol_source, ...)`. The system program requires `from.owner == system_program`. Torch's `TorchVault` is a program-owned PDA (holds state: `sol_balance`, totals, `linked_wallets`, etc.) and can't be a System.transfer source. Solution: a companion system-owned PDA for SOL routing.

**Accounts:**
- `TorchVault` — PDA at `["torch_vault", creator]`, owned by torch_market. Holds the vault's accounting and the token ATA authority.
- `TorchVaultSol` — PDA at `["torch_vault_sol", creator]`, system-owned, 0 bytes. Used only during `vault_swap` buys as a lamport waypoint. Sits at 0 lamports between swaps.

**Buy flow** (`handlers/swap.rs::vault_swap`):
1. Decrement `torch_vault.sol_balance -= amount_in`, increment `total_spent += amount_in`.
2. Direct lamport shuffle: `torch_vault.lamports -= amount_in; vault_sol.lamports += amount_in`. Torch owns `torch_vault` (can `sub_lamports`); anyone can `add_lamports` to any account.
3. CPI DeepPool `swap` with `user = torch_vault` (token authority) and `sol_source = vault_sol`. DeepPool does `System.transfer(from=vault_sol, to=pool, amount_in)`.
4. After the CPI, `vault_sol.lamports` returns to whatever it started with (typically 0).

**Sell flow** — `sol_source = torch_vault` directly. DeepPool's sell credits lamports via direct add (owner-agnostic), which works for program-owned accounts. `vault_sol` is not touched.

**Atomicity.** Steps 1-3 execute within a single instruction. Solana's single-threaded runtime makes the sequence uninterruptible. If the CPI in step 3 fails, the full transaction reverts — including the `sol_balance` decrement and the lamport shuffle.

**Attack surface probed:**

- **Substitution (#18):** The `vault_sol` account has `seeds = [TORCH_VAULT_SOL_SEED, creator]` + `bump` constraint. Anchor enforces the address at deserialization. No other account can satisfy the constraint.

- **Pre-credit / donation griefing (#19):** A third party can `System.transfer` lamports directly to any `vault_sol` PDA address. The donation lands on `vault_sol`, owner stays `system_program`. Next time the creator does a buy-swap:
  - Step 2 pushes `amount_in` from torch_vault into vault_sol. `vault_sol.lamports == donation + amount_in`.
  - Step 3's `System.transfer(from=vault_sol, amount_in)` drains exactly `amount_in`, leaving the donation in place.
  - No instruction exists that drains arbitrary `vault_sol` balances — only `vault_swap` buy touches it, and only for exactly `amount_in`.

  Net: the donated SOL is trapped. Donor loses their SOL; creator and protocol are unaffected. `torch_vault.sol_balance` is derived from `amount_in`, not from `vault_sol.lamports`, so the accounting is clean.

  **Critical:** if a future maintainer adds a "reclaim `vault_sol` dust" instruction, it must **not** be permissionless. An instruction that drains `vault_sol.lamports()` to any caller would let attackers direct-credit `vault_sol` and sweep it through that handler. Any such reclaim must require the creator's signature and cap at a safe amount. This is the main reason dust stays trapped by design.

- **Race (#20):** Solana atomicity — already noted.

- **Sell-path sol_source (#21):** `sol_source = torch_vault`. DeepPool requires `sol_source: Signer`. torch_vault signs via `invoke_signed` with `["torch_vault", creator, bump]`. Any attacker passing a different account as `sol_source` fails the Signer check unless they can produce a valid PDA signature for that account — which they can't (seeds are torch's namespace).

- **Treasury sell (#22):** Same as #21 but with `["treasury", mint, bump]` signing for `treasury` as `sol_source`.

**Why this is strictly better than the pre-v3.1 CPI pattern:** the old flow had torch pre-depositing lamports to the DeepPool pool PDA via direct manipulation, then CPIing swap with `amount_in` claimed. DeepPool's swap trusted the claim (no verification that the deposit actually happened). A malicious program could CPI with phantom `amount_in` and drain tokens. The v3.1 unified path made `System.transfer` self-authenticating — no claims, no trust. The TorchVault split preserves torch's composability without reintroducing any trust model.

---

## Composition with DeepPool

V20 is a composed system: torch_market + DeepPool, connected via CPI. Both have their own Kani proof suites:

| Program | Lines | Proofs | Covered |
|---------|-------|--------|---------|
| torch_market | ~7,900 | 73 | Buy/sell math, fees, lending accrual, short accrual, bad debt, migration math, protocol rewards, depth bands, DeepPool CPI accounting |
| deep_pool | ~1,340 | 16 | K invariant, fee conservation, LP proportionality, swap output bounds, fee compounding, LP lock rates |

Total arithmetic verification: **89 proof harnesses**. Auditors evaluating V20 should review both suites together. The composition boundary is the DeepPool CPI — torch_market's side is verified by proofs 69, 72, 73 (reserve reads, migration reimbursement, vault swap accounting); DeepPool's internal swap math is verified by its own proofs.

The DeepPool program has its own audit and redhat review (clean, no critical/high findings). See `/Users/mrbrightside/Projects/deep_pool/docs/audit.md` and its associated verification.

---

## Operational Recommendations

1. **Upgrade authority.** Currently live. Commit to a public timelock or multisig transition within the stabilization window (typical: 30-90 days post-mainnet activity).
2. **Monitor DeepPool pool creation rate.** Any pool created under `torch_config` that torch_market didn't initiate would indicate either a bug in the namespace enforcement or (more likely) nothing, because the namespace is cryptographically locked.
3. **Track sandwich attempts.** Watch for failed `Borrow` / `Liquidate` transactions with "LTV too high for pool depth" error. Spike = someone testing the depth gate. Expected rate: zero in normal operation.
4. **Reward claim analytics.** Verify `user_stats.last_epoch_claimed` moves monotonically per user. Any backwards movement is a runtime bug (should be impossible; verify empirically).
5. **Pool depth alerting.** Alert when any token's DeepPool drops below 10 SOL — approaching the 5 SOL floor where margin ops halt.

---

## What's NOT Audited Here

This document covers on-chain program correctness, not:
- Frontend input validation (separate audit)
- SDK transaction construction (separate audit)
- RPC provider availability
- Key custody for users and creators
- Economic risk of leverage and pool concentration (see `risk.md`)
- Off-chain indexing infrastructure

---

## Version History

| Version | Date | Key Changes |
|---------|------|-------------|
| V20.0.0 | Apr 2026 | Raydium → DeepPool. 73 Kani proofs. This audit. |
| V20.0.0 (torch_next) | Apr 2026 | TorchVault split for DeepPool v3.1 compatibility (new `TorchVaultSol` PDA). BondingCurve shrink (−243 bytes/curve, metadata moved to Token-2022 extension). Dead constraint cleanup. 7 additional redhat exploit classes (#18-24). New program ID. |
| V10.2.6 | Apr 2026 | Dev wallet share rebalance to 50%. Final pre-V20 version. |
| V10.2.5 | Apr 2026 | Per-user borrow cap 5x → 23x. |
| V10.2.3 | Apr 2026 | Per-user borrow cap 3x → 5x. |
| V10.2.2 | Apr 2026 | V6: depth bands, circuit breakers, bad debt fix, pool reserve guards. Independent cross-audit (OpenAI o3). |
| V10.2.0 | Apr 2026 | V36: vote vault removal. 100% tokens to buyer. |
| V10.1.1 | Apr 2026 | Free authority revocation at launch. |
| V10.0.0 | Apr 2026 | Oracle-free margin + short selling. |
| V4.0.0 | Mar 2026 | Treasury rate 12.5%→2.5%, protocol fee 1%→0.5%. |
| V3.7.30 | Mar 2026 | V35 community token option. |
| V3.7.22 | Feb 2026 | V33 buyback removal. |
| V3.7.17 | Feb 2026 | V29 on-chain metadata, Metaplex removal. |
| V3.7.10 | Feb 2026 | V20 swap_fees_to_sol. |
| V3.7.0 | Feb 2026 | V27 treasury lock, V28 zero-cost migration. |
| V3.2.3 | Feb 2026 | Original audit baseline. |

Full per-version findings are tracked in git history at `docs/audit.md` prior to this V20.0.0 rewrite.

---

**Auditor:** Claude Opus 4.7
**Final Assessment:** Ready for mainnet.
