# V20 Audit Report (current)

Independent adversarial review of the live v20 branch (deep_pool integration, lending unlock, gross-up, absolute per-user cap). Two scopes: the on-chain program (`programs/torch_market`) and the indexer + Postgres stack (`indexer/`).

## Summary

**No critical or high findings. One medium found and fixed during the audit (V20C-1, lending unlock gate). Six informational notes — see Findings.**

The v20-current surface adds three structural pieces beyond v20.0.0:
1. Treasury-gated lending unlock (`MIN_TREASURY_SOL_FOR_LENDING`, feature-flag tiered).
2. Absolute per-user borrow ceiling (`MAX_USER_BORROW_SHARE_BPS = 2000`) clamping the formula cap.
3. Token-2022 gross-up on every short close + liquidate so the 300M `TreasuryLock` is preserved.

Plus the unified per-user short cap (flat `MAX_WALLET_TOKENS`, 2% of supply), matching the bonding-curve anti-whale policy.

Adversarial coverage focused on the new surface: gate bypass paths, gross-up arithmetic, cap interactions with via_vault contexts, error-code shift impact, and indexer-side ingestion integrity.

## V20-current Findings

### V20C-1 — Lending unlock gate counted short collateral [MEDIUM, FIXED]

`check_borrow_caps` originally gated on `treasury.sol_balance >= MIN_TREASURY_SOL_FOR_LENDING`. Because `sol_balance` includes `short_collateral_reserved` (every `open_short` adds collateral to both fields), a single 100 SOL short on a fresh post-migration token cosmetically unlocked lending against an earned float below the threshold.

The attacker themselves gained nothing — short collateral is already excluded from `max_lendable` (utilization cap math one line below), so the attacker's parked 100 SOL was never lendable. What the bypass enabled was **early access to the protocol's organically-earned float** for any borrower while the gate's intent was "wait for activity-driven SOL to actually clear the threshold."

**Fix:** `lending.rs:check_borrow_caps` now computes `available_sol = sol_balance − short_collateral_reserved` once and uses it for both the gate check and the utilization cap. Same expression as before, lifted up.

**Regression coverage:**
- Kani: `verify_lending_gate_excludes_short_collateral` (proves gate state is invariant under any short collateral movement).
- litesvm: `borrow_gate_excludes_short_collateral` (sets `sol_balance = 150 SOL`, `short_reserved = 100 SOL`, asserts borrow still fires `LendingNotYetUnlocked`).

SDK + frontend mirror the same accounting: `LendingInfo.treasury_sol_available_lamports` exposes the gate-comparable value, and `LendingDashboard.tsx`'s progress bar reads it directly.

### V20C-2 — Short pool leaks below interest-coverage threshold [MEDIUM, FIXED]

Original (now-corrected) reading: the close-side gross-up makes the lock monotonic non-decreasing. **That claim was wrong**, and reopened when devnet testing showed the lock at 299.99M after a 30-minute open+close cycle (~10K display tokens leaked).

Real math: per cycle `lock_change = −fee_on_open + interest_accrued`. Solving `interest = fee_on_open` at the default 2%/epoch rate gives `T_breakeven ≈ 0.035 epochs ≈ 5.88 hours`. For shorts held **less than ~6 hours**, the gross-up only covers the close leg; the open-leg fee leaks. Sustained sub-6hr cycling would erode the lock over time.

**Fix:** `open_short` and `open_short_via_vault` now record `position.tokens_borrowed = args.tokens_to_borrow` (the gross sent from the lock), not the net received by the shorter. The borrower must close back the full gross + interest, with the close-side gross-up applied. Lock cycle change becomes `+interest_paid` regardless of hold duration.

Borrower impact: under the old design, the open-leg fee was implicit ("you asked for N tokens, got N − fee"). Under the new design, the borrower owes back the full N + interest, paying the open-leg fee gap at close time. Round-trip economic cost is identical (~0.14%); allocation just shifts from protocol-absorbed to borrower-paid.

**Regression coverage:**
- Kani: `verify_short_open_records_gross_amount` (gross recorded, not net) and `verify_short_full_close_lock_conservation` (lock balance non-decreasing across full cycle).
- litesvm: 20 `short::*` tests updated to assert gross. All passing.

SDK + frontend: `getShortPosition` returns `total_owed_tokens = tokens_borrowed + interest` unchanged; the field is now gross-based on-chain, which the close-short UI already displays via `grossUpForTransferFee(total_owed_tokens)` — no UI logic change needed, only the numeric values shift by ~0.07%.

### V20C-3 — Open-short has no lending unlock gate [INFORMATIONAL]

By design. Shorts borrow tokens from the static 300M `TreasuryLock`, not from SOL treasury. The lending unlock specifically gates the SOL lending side. Pre-unlock, shorts are the primary mechanism for growing the SOL float that ultimately unlocks longs (every short cycle adds 4× transfer fees + interest to the harvest pool). The asymmetry is intentional and documented in [risk.md](./risk.md) §4.3.

### V20C-4 — Gross-up ceiling never underpays the recipient [INFORMATIONAL]

`gross_up_for_transfer_fee` uses ceiling division: `gross = ceil(net × 10000 / (10000 − fee_bps))`. The Token-2022 withhold also rounds up. Net delivered to the lock is therefore always `≥ net`, with overshoot bounded by 1 unit. Kani `verify_gross_up_preserves_net_delivery` proves both bounds.

`programs/torch_market/src/math.rs:116-124` (function), `kani_proofs.rs::verify_gross_up_preserves_net_delivery`.

### V20C-5 — Absolute per-user cap dominates for any meaningful holder [INFORMATIONAL]

`calc_user_borrow_cap` returns `min(formula_cap, absolute_cap)`. The crossover point is `c/S = β/(10000·μ) = 2000/(10000·23) ≈ 0.870%` of supply. Any holder above that threshold hits the 20% absolute clamp first. Since the bonding-curve wallet cap is 2% of supply, every meaningful holder (anyone above the 0.87% crossover) is clamped by the absolute term. Design-correct: the formula cap protects against many-small-holders concentration; the absolute cap is the hard ceiling for whales.

`programs/torch_market/src/math.rs:337-353` (function), `kani_proofs.rs::verify_calc_user_borrow_cap_concrete_share`.

### V20C-6 — Open-short does not gross up [INFORMATIONAL]

By design (and required for the V20C-2 fix to work). On open, `treasury_lock` sends `args.tokens_to_borrow` (gross), the borrower receives `net = gross − fee` in their wallet, and `position.tokens_borrowed = gross`. Lock loses `gross`; borrower received less than the recorded debt. On close, borrower pays back `gross_up(gross + interest)`; lock receives `gross + interest` net. Cycle effect on lock: `+interest`, conservation invariant holds.

Could the open-leg also be grossed up so the lock sends `gross + open_fee_top_up`? No — that would require pulling extra tokens from somewhere, and the lock has no source for them. The borrower funds the open-leg fee implicitly at close, which is the only economically coherent mechanism.

`programs/torch_market/src/handlers/short.rs:248-285` (open_short), `:445-540` (close_short).

## V20.0.0 Findings (still applicable)

The following five informational notes from the v20.0.0 audit remain accurate for v20-current. No fixes warranted; documenting for completeness.

### V20-1 — MAX_WALLET cap is per-ATA, not per-controller [informational]

`quote_buy_tokens` caps `dest_balance + tokens_out ≤ MAX_WALLET_TOKENS`. Each ATA has its own cap. A user with multiple vaults could accumulate beyond 2% supply across vault ATAs. Same pre-v20 evasion path (multiple wallets), no new attack surface.

`programs/torch_market/src/handlers/market.rs:91-106`

### V20-2 — Liquidation principal/interest attribution is approximate in the bad-debt interest-only branch [informational]

In `apply_liquidation_loan_updates`, when `actual_debt_covered ≤ accrued_interest` AND `bad_debt > 0`, the deficit is subtracted from `borrowed_amount` and `accrued_interest` is forcibly zeroed. `total_sol_lent` is reduced by the same amount so on-chain accounting is self-consistent, but `total_sol_lent` slightly understates remaining principal in this edge case.

`programs/torch_market/src/handlers/lending.rs:685-720`
`programs/torch_market/src/handlers/short.rs:707-740`

### V20-3 — `liquidation_close_bps = 10000` could strand SOL collateral on shorts [informational]

If admin sets `treasury.liquidation_close_bps = 10000`, a single liquidation could fully cover debt while leaving surplus collateral. `CloseShort` requires `short_position.tokens_borrowed > 0`, so the borrower can't retrieve it. Default is 5000 (safe). Recommendation: if an admin setter is ever added, enforce `close_bps < 10000`.

### V20-4 — Vault-via-vault user attribution is per-controller [design note]

Volume/positions track by controller wallet, not by vault. `claim_protocol_rewards_via_vault` credits controller's volume but pays the SOL to the vault. Consistent with `link_wallet` semantics.

### V20-5 — Token-2022 fee leakage on short round-trips is self-correcting [informational]

Per-cycle, the lock breaks even on principal (gross-up) and gains interest tokens. The withhold pool accumulates fees that `harvest_fees` + `swap_fees_to_sol` eventually convert to SOL treasury. Closed-loop, no state-vs-runtime drift.

`programs/torch_market/src/handlers/short.rs:63-98`

## Verified Properties (no findings)

### Per-context constraints (40 instructions)
- Every `*ViaVault` context has the mandatory triple `torch_vault` + `vault_wallet_link` + `vault_token_account` (no Optional, no `as_ref().unwrap()`).
- `vault_wallet_link.vault == torch_vault.key()` cross-check present on all 10 via_vault contexts.
- `vault_wallet_link` PDA seeded by signer wallet, preventing cross-controller attacks.
- D-2 defense-in-depth (`bonding_curve.migrated`, `!bonding_curve.reclaimed`) present on all post-migration handlers.

### Helper consistency
Every split pair calls identical shared helpers (`compute_*`, `quote_*`, `finalize_*`, `apply_*`) with identical args sourced from corresponding accounts. Only differences are SOL funding source, token destination, and vault SOL accounting. No diverging math.

### Global invariants
- `treasury.sol_balance` always matches actual treasury PDA lamport flows (minus `star_sol_balance` carve-out).
- Lending state machine: `total_sol_lent`, `total_collateral_locked`, `active_loans` track in lockstep with `LoanPosition` state.
- Shorts state machine: `total_tokens_lent`, `active_positions`, `short_collateral_reserved` mirror `ShortPosition` aggregates.
- Vault SOL invariant: `sol_balance = total_deposited + total_received − total_withdrawn − total_spent`.

### Interest accrual
- `apply_interest_accrual` and `apply_short_interest_accrual` advance `last_update_slot` on every path (zero-debt, zero-elapsed, normal) — Kani-proven.
- No stale-slot bug on re-borrow of a fully-repaid-but-not-closed position.

### Gate semantics
- `available_sol = sol_balance − short_collateral_reserved` is the single source of truth for both the unlock gate and the utilization cap (V20C-1 fix).
- Opening or closing a short of any size is invariant to the gate state.
- Build-feature `simnet` + `devnet` together is a `compile_error!`, preventing accidental dual-flag builds.

### CPI surface
- All `CpiContext::new_with_signer` sites use correct PDA seeds.
- No re-entrancy possible: CPIs go only to Token-2022, System, DeepPool, Associated Token Program — none call back into torch_market.
- Token-2022 fee handling uses reload-and-diff for inflows; gross-up for outflows that must arrive net-whole at the lock.

### Adversarial split-specific
- Anchor discriminators prevent wallet-ix accepting via_vault accounts and vice-versa.
- Mix-and-match (vault A's torch_vault + vault B's link) blocked by `vault_wallet_link.vault == torch_vault.key()`.
- Cross-controller link attacks blocked by `vault_wallet_link` PDA seeded by signer.
- Borrow with `collateral = 0` rejected by `calc_ltv_bps` division-by-zero guard.

## Indexer + Postgres Audit

The indexer ingests torch + deep_pool events from Helius Laserstream (Yellowstone gRPC over TLS) and persists them to Postgres. The on-chain protocol's correctness is independent of the indexer — the program is the source of truth, the indexer is a read replica. But because the indexer is what the SDK/frontend reads for market data, price history, and PnL, its integrity matters for user-facing accuracy.

**Scope:** can an attacker poison the indexer's view of the world such that the frontend shows fake prices, fake positions, or fake trades? Can an attacker corrupt the DB directly?

### Findings

**No exploitable findings.** The defense surface is layered and the role separation is strict.

### Deployment surface

- **Postgres:** `127.0.0.1:5432` host binding (loopback only). Inside the compose network, the indexer reaches it via service-name DNS (`postgres:5432`).
- **Indexer API:** `127.0.0.1:8080` host binding (loopback only). Production deployment must reverse-proxy with rate limiting + CORS allowlist if exposed publicly.
- **CORS:** `CorsLayer::permissive()` in `api.rs`. Acceptable because the API is loopback-only; if exposed publicly without a reverse proxy enforcing CORS, this becomes meaningful.

`indexer/docker-compose.yml`

### Role separation

The Postgres role used by the indexer (`torch_indexer`) is least-privilege:

- **Login + connect** to the `torch` database, `USAGE` on `public` schema.
- **`SELECT, INSERT, UPDATE`** on event + state tables (markets, trades, messages, loans, shorts, migrations, pools, reserves, swaps, liquidity_events, indexer_state, metadata_fetch_log).
- **No `DELETE`, no `TRUNCATE`, no `CREATE`**. Schema-modifying rights explicitly revoked.
- **`USAGE` on all sequences** (required because `INSERT` doesn't implicitly grant `nextval()`).

The superuser role (`torch`) is used only for init + ops, never by the indexer process. Credentials are separate env vars (`POSTGRES_PASSWORD` vs `INDEXER_DB_PASSWORD`).

`indexer/db/02-indexer-role.sh`

### Data-source authenticity

The indexer's only write path is the gRPC ingestion pipeline:

1. Helius Laserstream gRPC over TLS — authenticated with `LASERSTREAM_TOKEN`.
2. Subscription filtered by `account_include = [torch_program_id, deep_pool_program_id]`.
3. Inner instructions are decoded only if their `program_id_index` resolves to torch or deep_pool program bytes.
4. Payload must start with `EVENT_IX_TAG_LE` (the Anchor `emit_cpi!` 8-byte tag) before discriminator dispatch — non-event CPIs are silently skipped.
5. Discriminator must match a known event name; payload must Borsh-deserialize cleanly with no trailing bytes.

An attacker would need to either (a) compromise Helius credentials, (b) get the on-chain torch program to emit fake events (impossible without controlling the program), or (c) intercept TLS in flight. None of these paths exist as practical attacks.

`indexer/src/stream/grpc.rs:240-334`, `indexer/src/stream/decoder.rs`

### SQL injection surface

All query construction uses sqlx with parameter binding:

- API handlers in `api.rs` use `query_as`/`query` with `.bind()` for every value. No `format!` or string interpolation into SQL.
- Domain layer uses `QueryBuilder<Postgres>` with `.push_bind()` for dynamic WHERE clause construction (filters by mint, creator, status, etc.). All user-controlled values go through `push_bind`.
- The only raw SQL is the candles aggregation (`api.rs:list_candles`), which uses `.bind(&q.mint).bind(since).bind(before).bind(bucket_seconds)`. The `interval` string is matched against a closed allowlist (`1s | 15s | 30s | ...`) before deriving `bucket_seconds`; no user string reaches SQL.

Verified by grep across `indexer/src/`: `format!` is absent from query construction; `QueryBuilder` and `query_as` are the only paths.

`indexer/src/api.rs`, `indexer/src/domain/*.rs`

### Idempotency + replay safety

Every event insert is idempotent on `(signature, inner_ix_idx)`:

- `ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *` on append-only tables (trades, messages, swaps, liquidity_events, reserves).
- `ON CONFLICT (mint) DO NOTHING` on identity-keyed state tables (markets, migrations).
- `ON CONFLICT (mint, borrower) DO UPDATE` on current-state position tables (loans, shorts).
- `ON CONFLICT (pubkey) DO NOTHING` on pools.

Per-request reads use `REPEATABLE READ` isolation (`RequestCtx::begin`). The writer task processes one BlockBatch per Postgres transaction; checkpoint advances only on commit, so partial writes never leak.

`indexer/src/domain/*.rs`, `indexer/src/services/context.rs`

### Read-only API surface

Every HTTP route in `api.rs` is `GET`. No write endpoints exist. Even if the API role's `INSERT`/`UPDATE` privileges were exploited via a hypothetical injection (none found), there is no code path that takes untrusted input and writes it.

WS `/events` is a fan-out of in-process broadcast frames from the writer task. Subscribers receive what the writer already committed; no client message can influence what gets written.

`indexer/src/api.rs:52-76`

### Recommendations for production deployment

These aren't findings — they're hardening steps that move beyond the code into ops posture:

1. **Reverse proxy** the indexer API (nginx, Caddy, or Cloudflare) with rate limiting (~50 req/s per IP for `/api/*`, more permissive for `/healthz`) and a CORS allowlist scoped to the frontend domain.
2. **TLS termination** at the reverse proxy. The indexer binds to `127.0.0.1:8080` plaintext — never expose this directly.
3. **Distinct passwords per environment.** Devnet, mainnet, and any preview environments should use unrelated secrets for both `POSTGRES_PASSWORD` and `INDEXER_DB_PASSWORD`.
4. **Backups + PITR.** The indexer is replayable from chain (it can backfill), but backups speed recovery and protect against operator error.
5. **Network isolation.** Production indexer should run in a private subnet; only the reverse proxy host needs access to it.
6. **Monitor `block_write_errors_total` + `indexer_state.last_processed_slot`.** Alerts on growing checkpoint lag catch ingestion stalls before they cascade to stale UI data.
7. **Rotate `LASERSTREAM_TOKEN` periodically.** Treat as a credential, not a config value.

## Verdict

V20-current is safe to deploy. The audit found one real semantic issue (V20C-1, gate bypass via short collateral) which has been fixed with both a Kani harness and a litesvm regression test. The remaining notes are accounting nuances or admin-misconfiguration edge cases, not exploits.

The indexer + Postgres stack has a strong security posture: loopback-only bindings, least-privilege DB role, authenticated TLS upstream, no SQL injection surface, no write API, idempotent inserts. The recommendations above are deployment hardening, not code fixes.

**Total verification coverage:**
- 84 Kani proof harnesses (program math, state transitions, CPI accounting, gate semantics)
- 42 proptest properties × 5,000 cases (random-input math sweep)
- 105 litesvm integration tests (BPF execution against real .so binaries)
- DeepPool composition: 16 additional Kani proofs (swap math, fee invariants, LP proportionality) — total composed system: 100 proof harnesses.
