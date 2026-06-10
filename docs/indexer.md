# Torch Indexer

## Goal

Capture every state-mutating event from `torch_market` and `deep_pool` via Yellowstone gRPC, persist to a single Postgres, expose via HTTP/WS. Market lists, price charts, message boards, holder counts, and active-margin-positions become fast indexed reads. `torchsdk` consumers can opt in with `ReadOptions { indexer }` per call; without it, every read path falls back to RPC.

The indexer is an **acceleration layer, not a dependency**. The SDK must remain fully functional against RPC alone. Only paths that need historical aggregation across many transactions (`getTrades`, `getCandles`) are indexer-only — RPC cannot answer them in reasonable time.

## Architecture

Single Rust crate, two program ingestion streams, one DB:

```
                  ┌──────────────────────┐
                  │  Yellowstone gRPC    │
                  │  subscribed to:      │
                  │  - torch_market PID  │
                  │  - deep_pool PID     │
                  └──────────┬───────────┘
                             │ slot-ordered events (both programs interleaved)
                             ▼
        ┌──────────────────────────────────────────┐
        │  Decoder (per-program)                   │
        │   torch::decode → MarketCreated, Buy,    │
        │     Sell, OpenShort, ..., MigrateToDex   │
        │   deep_pool::decode → PoolCreated,       │
        │     SwapExecuted, LiquidityAdded, ...    │
        └────────────────────┬─────────────────────┘
                             │
                             ▼
        ┌──────────────────────────────────────────┐
        │  Writer (one Postgres txn per block)     │
        │   torch tables ∪ deep_pool tables ∪      │
        │   indexer_state.last_processed_slot      │
        └──────────────────┬─────────┬─────────────┘
                           │ COMMIT  │
                           ▼         ▼
                    ┌──────────┐ ┌──────────────┐
                    │ WS       │ │ axum HTTP    │
                    │ /events  │ │ /api/*       │
                    │ firehose │ │ (REPEATABLE  │
                    │ broadcast│ │  READ per    │
                    │ post-    │ │  request)    │
                    │ commit   │ └──────────────┘
                    └──────────┘
```

**Stream side** (`stream/`): Yellowstone → per-program decoder → writer. Decoders are isolated by program — torch events never touch deep_pool tables and vice versa, except via the migration FK linking `markets.deep_pool_pubkey` → `pools.pubkey`.

**Subscription note (2026-06-09: the code subscribes to BLOCK updates and iterates `block.transactions` in chain order — `stream/grpc.rs` is the source of truth; the note below describes an earlier mid-migration state):** Use `SubscribeRequestFilterTransactions`, not `SubscribeRequestFilterBlocks`, for "subscribe to all activity touching program X". The blocks filter delivers block shells with transactions filtered inside, but program-touching txs don't reliably surface in the delivered block's `transactions` field. The transactions filter is Helius's documented canonical pattern and delivers each matching tx as its own `UpdateOneof::Transaction`. See `stream/grpc.rs::subscribe_once`. Symptom of getting this wrong: `blocks_written_total` rises at network rate (the writer commits empty batches) but `events_total` stays at zero forever — looks alive, sees nothing.

**API side** (`api.rs` + `services/` + `domain/`): axum, per-request `REPEATABLE READ` transaction, lazy-loaded service composition. Direct copy of the deep_pool pattern.

## Schema

All event tables carry `(signature, inner_ix_idx) UNIQUE` so backfill can replay without dupes.

### Torch tables

```
markets
  mint (PK), name, symbol, metadata_uri, image_url, creator,
  status (BONDING | COMPLETE | MIGRATED | RECLAIMED),
  tier (flame | torch), sol_target,   -- spark (50 SOL) removed from the program
  virtual_sol, virtual_token, real_sol, real_token,
  bonding_complete_slot, migrated_slot, last_activity_slot,
  deep_pool_pubkey (nullable, FK → pools.pubkey post-migration),
  created_at, updated_at
  indexes: (creator, created_at), (status, real_sol DESC)

trades
  mint, trader, is_buy, sol_in, sol_to_treasury, protocol_fee,
  creator_fee, tokens_out,
  virtual_sol_after, virtual_token_after, real_sol_after,
  slot, signature, inner_ix_idx, created_at
  indexes: (mint, slot DESC), (trader, slot DESC)

messages
  mint, sender, memo_text, slot, signature, inner_ix_idx, created_at
  indexes: (mint, slot DESC), (sender, slot DESC)

loans
  mint, borrower, collateral_amount, borrowed_amount,
  accrued_interest_stored, last_update_slot, health (snapshot),
  is_active, updated_at
  composite PK: (mint, borrower)
  indexes: (mint, is_active, health, updated_at DESC)

shorts
  mint, shorter, sol_collateral, tokens_borrowed,
  accrued_interest_stored, last_update_slot, health (snapshot),
  is_active, updated_at
  composite PK: (mint, shorter)
  indexes: (mint, is_active, health, updated_at DESC)

migrations
  mint (PK), deep_pool_pubkey, lp_burned, slot, signature, created_at
```

### Deep_pool tables (verbatim from existing schema)

`pools`, `swaps`, `liquidity_events`, `reserves` — no changes. The torch indexer writes these in the same transactions that mutate torch tables when an event touches both programs (e.g., `migrate_to_dex` writes `markets.migrated_slot` + `markets.deep_pool_pubkey` AND inserts into `pools` and `reserves`).

### Cross-cutting

```
indexer_state
  id INT PK CHECK (id = 1)
  last_processed_slot BIGINT NOT NULL

metadata_fetch_log
  mint (PK), metadata_uri, last_fetched_at, status (ok | 404 | error),
  image_url (cached after successful fetch)
  Used to avoid re-fetching dead arweave URIs every restart.
```

## V21 Leverage Migration

**Status:** IMPLEMENTED (2026-06-09) — decoders, positions/position_events tables, writer reconcile, API. This section is the design record. **Supersedes** the V20 `loans`/`shorts` tables and the `Loan*`/`Short*` decoders in the Schema section above — those are stale and will be replaced.

### Why

V21's closed-loop-leverage rework renamed **and** restructured all six leverage events:

| V20 event (current decoder) | V21 event (program emits) |
|---|---|
| `LoanCreated` / `LoanRepaid` / `LoanLiquidated` | `OpenLongEvent` / `CloseLongEvent` / `LiquidateLongEvent` |
| `ShortOpened` / `ShortClosed` / `ShortLiquidated` | `OpenShortEvent` / `CloseShortEvent` / `LiquidateShortEvent` |

Anchor discriminator = `sha256("event:<Name>")[..8]`, so the rename alone makes every leverage discriminator miss — **leverage is currently unindexed** (shorts/longs/liquidations are invisible). Three structural changes drive the schema:

1. **Unified `Position`.** On-chain, longs and shorts are one `Position` struct with `side: Long|Short`. Long = token collateral / SOL debt; short = SOL collateral / token debt.
2. **`position_index` (`u32`).** A user can hold MANY positions per `(user, mint)`. The V20 PKs `(mint, borrower)` / `(mint, shorter)` collide → the index must be in the key.
3. **via_vault ownership.** The 6 `*_via_vault` handlers emit the SAME events, but `user`/`borrower` is a `torch_vault` PDA, not a wallet — same shape, vault owner.

### Decision: one `positions` table (mirror on-chain), not separate `loans`/`shorts`

V21 unified the on-chain model under `Position`; the indexer mirrors it. A single table makes "all positions for a user" one query (vs a UNION), keeps the schema aligned with the program's own generic `collateral_amount`/`debt_amount` fields (units interpreted by `side`, exactly as on-chain), and folds via_vault in transparently. Cost: `collateral_amount`/`debt_amount` are unit-ambiguous (tokens vs lamports by side) — a documentation matter, and the API can expose typed views. *(Alternative considered: keep separate, typed `loans`/`shorts` tables — clearer columns but duplicates structure and diverges from the unified on-chain model. Rejected for the mirror.)*

### Schema

```
CREATE TYPE position_side AS ENUM ('long','short');
CREATE TYPE position_event_kind AS ENUM ('open','close','liquidate');

-- current-state, UPSERTed per event (the audit row; never deleted)
positions
  mint              TEXT NOT NULL REFERENCES markets(mint),
  owner             TEXT NOT NULL,          -- wallet OR torch_vault PDA
  side              position_side NOT NULL,
  position_index    INT  NOT NULL,
  collateral_amount BIGINT NOT NULL,        -- long: tokens   | short: lamports
  debt_amount       BIGINT NOT NULL,        -- long: lamports | short: tokens
  open_fee_sol      BIGINT NOT NULL,
  vault_balance     BIGINT NOT NULL,        -- per-position vault: long=tokens, short=lamports
  accrued_interest_stored BIGINT NOT NULL,
  last_update_slot  BIGINT NOT NULL,
  health            position_health NOT NULL,   -- snapshot; API recomputes off the TWAP mark
  is_active         BOOLEAN NOT NULL,
  owner_is_vault    BOOLEAN NOT NULL,           -- set from the emitting ix variant (via_vault?)
  created_at, updated_at,
  PRIMARY KEY (mint, owner, side, position_index)
  indexes: (mint, side, is_active, health, updated_at DESC), (owner, is_active)

-- append-only event log (the leverage analog of `trades`)
position_events
  mint, owner, side, position_index,
  kind            position_event_kind,
  liquidator      TEXT NULL,                 -- liquidate only
  interest_paid, principal_paid, surplus_sol,    -- close
  sol_in, sol_out, tokens_in, tokens_out,        -- open/close legs
  bad_debt, twap_ltv, bonus_bps, seized, residual, fully_resolved,  -- liquidate (NULL otherwise)
  slot, signature, inner_ix_idx, created_at
  UNIQUE (signature, inner_ix_idx)
  indexes: (mint, slot DESC), (owner, slot DESC), (kind, slot DESC)
```

**Why the event log:** liquidation events expose `bad_debt`, `twap_ltv`, `bonus_bps`, `sol_seized`/`tokens_seized` — data that exists ONLY at event time and is lost under current-state-only. It's the liquidation feed + per-position P&L history (V20 indexed no leverage event log). `positions` answers "what's open now"; `position_events` answers "what happened."

### Field mapping (V21 event → rows)

| Event | `positions` upsert | `position_events` insert |
|---|---|---|
| `OpenShortEvent` {user,mint,index,collateral_sol_gross,open_fee_sol,net_collateral_sol,tokens_borrowed,vault_sol} | side=short, collateral=net_collateral_sol, debt=tokens_borrowed, open_fee_sol, vault_balance=vault_sol, is_active=true | kind=open, sol_in=collateral_sol_gross, tokens_out=tokens_borrowed |
| `OpenLongEvent` {user,mint,index,collateral_tokens,borrowed_sol_gross,open_fee_sol,atomic_buy_sol,vault_tokens} | side=long, collateral=collateral_tokens, debt=borrowed_sol_gross, open_fee_sol, vault_balance=vault_tokens | kind=open, tokens_in=collateral_tokens, sol_out=atomic_buy_sol |
| `CloseShortEvent` / `CloseLongEvent` {…,debt_repaid/…,interest_paid,principal_paid,surplus_sol_to_user,fully_closed} | debt/collateral decremented; is_active=!fully_closed | kind=close, interest_paid, principal_paid, surplus_sol, fully_resolved=fully_closed |
| `LiquidateShortEvent` / `LiquidateLongEvent` {liquidator,borrower,…,bad_debt,bonus_bps,twap_ltv,seized,residual,fully_liquidated} | debt decremented; is_active=!fully_liquidated | kind=liquidate, liquidator, bad_debt, twap_ltv, bonus_bps, seized, residual, fully_resolved=fully_liquidated |

`owner_is_vault` is set by the **emitting instruction variant** — the per-tx walk already inspects the torch ix (message-gating does this); a `*_via_vault` ix ⇒ `owner_is_vault = true`. (The event payload alone can't tell; the ix can.)

### Health: coarse snapshot now, TWAP recompute is a follow-up

V21's liquidation trigger is `twap_ltv` off the deep_pool TWAP mark. The on-chain program owns that trigger; the indexer is observability. **Implemented today:** the writer stores a coarse `health` snapshot (`open`/`close` → `healthy`, `liquidate` → `liquidatable`), and every liquidation's exact `twap_ltv` is preserved in `position_events`. **Follow-up:** a live API recompute that joins the active position to its `markets.deep_pool_pubkey` pool, reads the latest TWAP from the `reserves` snapshot, and applies V21 `calc_ltv_bps` to classify healthy / at_risk / liquidatable. Until then the `?health=` filter sorts on the stored snapshot, not a live mark.

### Code surface

- `contracts.rs`: drop the 6 `Loan*`/`Short*` structs + `LoanRow`/`ShortRow`/`New*Row`; add the 6 V21 event structs + `PositionRow`/`PositionEventRow`.
- `decoder.rs`: replace the 6 V20 discriminators/match arms with the V21 names.
- `translate.rs`/`writer.rs`/`domain`: upsert `positions` (key incl. `side` + `position_index`) + insert `position_events`; resolve `owner_is_vault` from the ix.
- `api.rs`: **retire** `/api/loans` + `/api/shorts`. `GET /api/positions ?mint&side&owner&health&is_active` replaces both; new `GET /api/liquidations` over `position_events WHERE kind='liquidate'`. SDK `getAllLoanPositions`/`getAllShortPositions` → `getPositions ?side`.

### Migration

Devnet / pre-mainnet ⇒ **clean replace**: drop `loans`/`shorts`, create `positions` + `position_events` + the two enums, re-backfill from genesis (or `START_SLOT`). No data migration needed.

## Ingest behavior

### Backfill

From genesis by default. Override via `START_SLOT` env var. Both program streams run in parallel; they write to disjoint torch/deep_pool tables (no contention). Each block boundary commits.

### Messages: gating

Memo program events are only persisted when the **same transaction** contains at least one instruction targeting `torch_market` PROGRAM_ID. This makes the message board per-market (think Craigslist by board, not personal inbox). Memos from unrelated programs are dropped.

Implementation: in `stream/decoder.rs`, the per-tx walk inspects all instructions; if any is a torch ix AND a memo ix is present, the memo is attributed to the torch-side mint (derived from the bonding curve account in the torch ix's accounts list).

### Migration atomicity

`migrate_to_dex` writes `BondingCurve.migrated = true` AND triggers `create_pool` on deep_pool in the same Solana transaction. The writer's per-block transaction sees both side effects together, so the torch row + deep_pool row + the FK in `markets.deep_pool_pubkey` are all set atomically. No half-state windows.

### Position snapshots

`positions` is a *current snapshot*, not an event log. Every `open_*`, `close_*`, `liquidate_*` (and their `_via_vault` variants) UPSERTs the position row keyed by `(mint, owner, side, position_index)`; open events carry absolute amounts, close/liquidate carry deltas the writer reconciles against the prior row. When a position fully resolves, `is_active = false` (we don't delete — the row is the audit trail). The append-only `position_events` log preserves the per-event analytics (`bad_debt`, `twap_ltv`, `bonus_bps`, `seized`, `residual`) that the current-state row can't retain. The `accrued_interest_stored` + `last_update_slot` fields let the API recompute current interest at request time.

### Token-2022 fee semantics

`trades.tokens_out` is the NET amount the trader actually received (matches `transfer_checked` destination delta). Consistent with the on-chain `position.tokens_borrowed` net-recording fix landed earlier — gross is never stored, since it doesn't reflect what anyone holds.

## API surface

```
GET  /healthz
GET  /api/markets             ?status&creator&tier&since&limit
GET  /api/markets/:mint       detail incl. enrichment + current reserves
GET  /api/trades              ?mint&trader&since&before&limit
GET  /api/messages            ?mint&sender&since&before&limit
GET  /api/positions           ?mint&owner&side&health&is_active   [V21] long+short unified (side=long|short)
GET  /api/liquidations        ?mint&owner&side&kind                [V21] position_events log (defaults kind=liquidate)
GET  /api/holders/:mint       count + top-N (joins markets to ATAs)
GET  /api/candles             ?mint&interval&since&before
                              interval ∈ {1m, 5m, 1h}
WS   /events                  firehose, all programs, post-commit
```

### Candles

On-demand SQL window query over `trades ∪ swaps WHERE mint = ?` ordered by slot, time-bucketed. Indexes on `(mint, slot DESC)` plus `time_bucket()` (or `date_trunc()` if not using TimescaleDB) make this fast for typical chart-window ranges. Promote to a materialized view only if measured latency forces it. **The user's hypothesis is that properly-indexed on-demand will be fast enough; we proceed under that assumption and revisit empirically.**

### WS firehose

Single broadcast channel, all events. Wire format:

```json
{
  "type": "trade" | "message" | "market_created" | "migration"
         | "loan_updated" | "short_updated" | "swap" | "liquidity",
  "mint": "...",                   // for torch events
  "pool": "...",                   // for deep_pool events
  "slot": 12345,
  "signature": "...",
  "payload": { /* event-specific */ }
}
```

Clients filter by `type` + `mint`/`pool`. The `/markets` page subscribes for live sort + progress bars; `/markets/[mint]` filters by single mint. Same pattern as deep_pool — no per-connection server-side filtering, broadcast is cheap relative to indexing.

## SDK wiring

`torchsdk` adds:

```ts
export interface ReadOptions { indexer?: string }

// Generic indexer-first wrapper, copied from deeppoolsdk
async function withFallback<T>(primary: () => Promise<T>, fallback: () => Promise<T>): Promise<T>
```

Per-getter classification:

| Getter | Mode |
|---|---|
| `getTokens` | indexer-first → `getProgramAccounts` |
| `getToken` | indexer-first → direct PDA read |
| `fetchMintsMetadata` | indexer-first → direct HTTPS fetch |
| `getHolders` | indexer-first → RPC `getTokenAccountsByMint` |
| `getMessages` | indexer-first → existing `getSignaturesForAddress` walk |
| `getAllLoanPositions` | indexer-first → existing program-accounts scan |
| `getAllShortPositions` | indexer-first → existing program-accounts scan |
| **`getTrades`** | **indexer-only** |
| **`getCandles`** | **indexer-only** |
| Quotes, transaction builders | unchanged — always RPC |

The `indexer` URL threads through `NetworkContext` so the frontend passes it once at the connection layer. Consumers integrating torchsdk without infrastructure get a fully functional SDK minus the historical chart data they wouldn't realistically query from RPC anyway.

## Decisions log

| # | Decision | Rationale |
|---|---|---|
| 1 | Single indexer ingests both `torch_market` and `deep_pool` events into one DB | Cleaner than monorepo or cross-DB FDW. Existing deep_pool standalone indexer becomes redundant in any environment running torch and can be retired. |
| 2 | Tables: torch's named per-domain, deep_pool tables kept verbatim from existing schema | Names are already domain-correct, no churn needed. |
| 3 | Backfill: genesis-by-default, optional `START_SLOT` cutoff | Devnet has churned a lot; cutoff lets fresh envs skip dead history. |
| 4 | Messages: gated on co-presence with a torch instruction in the same tx | Per-market message board model (Craigslist by board, not personal inbox). |
| 5 | WS: firehose broadcast, client-side filtering | Same as deep_pool. Markets page consumes for live sort; detail page filters by mint. |
| 6 | Candles: on-demand SQL window query for v1 | Indexes should make it fast. Materialized view only if metrics force it. |
| 7 | Token-2022 net semantics: indexer records NET amounts everywhere | Consistent with the on-chain `tokens_borrowed = net` fix. Gross doesn't reflect anyone's holdings. |
| 8 | Single `indexer_state.last_processed_slot` cursor | Yellowstone is slot-ordered globally across subscribed programs. |
| 9 | Indexer is acceleration only; every SDK read except `getTrades`/`getCandles` has an RPC fallback | Keeps torchsdk usable for AI agents, CLIs, third-party integrations without infrastructure. |
| 10 | **[V21]** Consolidate `loans`+`shorts` into one `positions` table (`side` enum, `position_index` in PK) + an append-only `position_events` log | Mirrors V21's unified on-chain `Position`; "all positions for a user" is one query; `position_events` preserves liquidation analytics (`bad_debt`/`twap_ltv`) that current-state-only would lose. See "V21 Leverage Migration". |
| 11 | **[V21]** Position health classified off the deep_pool TWAP mark, not the reserve ratio. **Today:** writer stores a coarse snapshot + every liquidation's exact `twap_ltv` lands in `position_events`. **Follow-up:** live API recompute via `calc_ltv_bps` against the latest TWAP. | V21's liquidation trigger is `twap_ltv`; classify against the same mark the program liquidates on. The program owns the actual trigger — the indexer is observability, so a coarse snapshot + preserved per-liquidation `twap_ltv` is sufficient until the live recompute lands. |

## Out of scope (v1)

- **Materialized candles** — until on-demand proves too slow.
- **Indexed image hosting (CDN/S3)** — `markets.image_url` stores the resolved URL; frontend fetches direct from arweave. Local cache layer is a follow-up if arweave outages become a problem.
- **Holder-distribution analytics** beyond count + top-N.
- **Cross-mint correlations / leaderboards.**
- **User-stats indexing** — current on-chain `UserStats` is bonding-only; the indexer's own `trades` and `swaps` aggregations will eventually supersede it, but for v1 we don't write a `user_stats` table.

## Addenda (2026-06-09 correctness pass)

- **Chain ordering [I-1]:** the writer sorts each block by `(tx_idx, inner_ix_idx)`
  (Yellowstone block position live; per-slot signature-walk ordinal on backfill).
  Serial ids are therefore intra-slot chain order; read queries tiebreak on id,
  never signature.
- **Lifecycle [I-4]:** every status transition rides a program event —
  `BondingCompleted` → COMPLETE, `MigratedToDex` → MIGRATED, `TokenReclaimed` →
  RECLAIMED, `TokenRevived` → BONDING. The indexer never derives state the
  program didn't announce. ASN removed (dead); RS/RD relabeled BONDING/COMPLETE.
- **Position reconcile [I-2]:** mirrors the on-chain Position exactly —
  `debt −= principal_paid`; `collateral_amount` static (audit);
  `vault_balance −= event deltas`; `accrued_interest_stored = prior +
  calc_interest(prior_debt, 150 bps, Δslots) − interest_paid`. Liquidations
  split interest-first like the program.
- **Backfill ≡ live [I-5]:** backfill seeds `lp_supply` from the DB (an empty
  cache wrote `lp_supply = 0` reserve snapshots).
- **Known accepted gaps:** `ts()` falls back to ingest time when `block_time`
  is absent (backfilled candles may shift; slot is the durable ordering);
  commitment is CONFIRMED — rows from a dropped fork are never deleted
  (idempotent keys make re-ingest safe; optimistic confirmation makes drops a
  slashing-grade event, near-zero probability). Decoder is
  STRICT (rejects trailing bytes): program event-layout changes deploy in
  lockstep with the indexer, by policy.
- **Test debt (deferred):** backfill-vs-live equivalence replay; donation-resync
  replay (deep_pool P-2); LiquidityRemoved decode roundtrip; reorg overlap
  re-ingest.

### Follow-up design: finalized-watermark janitor (revisit post-mainnet)

Ingest stays at CONFIRMED (no lag); truth repairs at FINALIZED (no trust).
The asymmetry that justifies it: a phantom row in an append log is a bounded
ghost; a phantom in a current-state table (e.g. a fork-dropped close) diverges
FOREVER. Design:

1. Track a finalized watermark alongside last_processed_slot.
2. Janitor pass over rows whose slot has crossed the watermark: batched
   getSignatureStatuses (256/call) on distinct signatures; signatures absent
   on-chain are fork phantoms → DELETE their rows across all tables.
3. Rebuild affected state from surviving events — position_events is an
   append-only WAL, so positions re-fold per (mint, owner, side, index);
   statuses re-derive from surviving lifecycle events. (The reorg-buffer
   resume already heals MISSED events; the janitor heals PHANTOM ones —
   together complete.)
4. Optionally expose the watermark via the API so consumers can distinguish
   settled from provisional rows.
