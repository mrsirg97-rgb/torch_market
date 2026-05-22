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

**Subscription gotcha**: Use `SubscribeRequestFilterTransactions`, not `SubscribeRequestFilterBlocks`, for "subscribe to all activity touching program X". The blocks filter delivers block shells with transactions filtered inside, but program-touching txs don't reliably surface in the delivered block's `transactions` field. The transactions filter is Helius's documented canonical pattern and delivers each matching tx as its own `UpdateOneof::Transaction`. See `stream/grpc.rs::subscribe_once`. Symptom of getting this wrong: `blocks_written_total` rises at network rate (the writer commits empty batches) but `events_total` stays at zero forever — looks alive, sees nothing.

**API side** (`api.rs` + `services/` + `domain/`): axum, per-request `REPEATABLE READ` transaction, lazy-loaded service composition. Direct copy of the deep_pool pattern.

## Schema

All event tables carry `(signature, inner_ix_idx) UNIQUE` so backfill can replay without dupes.

### Torch tables

```
markets
  mint (PK), name, symbol, metadata_uri, image_url, creator,
  status (RS | RD | ASN | MIGRATED | RECLAIMED),
  tier (spark | flame | torch), sol_target,
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

## Ingest behavior

### Backfill

From genesis by default. Override via `START_SLOT` env var. Both program streams run in parallel; they write to disjoint torch/deep_pool tables (no contention). Each block boundary commits.

### Messages: gating

Memo program events are only persisted when the **same transaction** contains at least one instruction targeting `torch_market` PROGRAM_ID. This makes the message board per-market (think Craigslist by board, not personal inbox). Memos from unrelated programs are dropped.

Implementation: in `stream/decoder.rs`, the per-tx walk inspects all instructions; if any is a torch ix AND a memo ix is present, the memo is attributed to the torch-side mint (derived from the bonding curve account in the torch ix's accounts list).

### Migration atomicity

`migrate_to_dex` writes `BondingCurve.migrated = true` AND triggers `create_pool` on deep_pool in the same Solana transaction. The writer's per-block transaction sees both side effects together, so the torch row + deep_pool row + the FK in `markets.deep_pool_pubkey` are all set atomically. No half-state windows.

### Position snapshots

`loans` and `shorts` are *current snapshots*, not event logs. Every `open_short`, `borrow`, `repay`, `close_short`, `liquidate`, etc. UPSERTs the position row. When a position fully closes, `is_active = false` (we don't delete — the row is the audit trail). The `accrued_interest_stored` field plus `last_update_slot` lets the API recompute current interest at request time using the existing `apply_*_interest_accrual` math (no separate "live interest" indexer pass needed).

### Token-2022 fee semantics

`trades.tokens_out` is the NET amount the trader actually received (matches `transfer_checked` destination delta). Consistent with the on-chain `position.tokens_borrowed` net-recording fix landed earlier — gross is never stored, since it doesn't reflect what anyone holds.

## API surface

```
GET  /healthz
GET  /api/markets             ?status&creator&tier&since&limit
GET  /api/markets/:mint       detail incl. enrichment + current reserves
GET  /api/trades              ?mint&trader&since&before&limit
GET  /api/messages            ?mint&sender&since&before&limit
GET  /api/loans               ?mint&health&is_active
GET  /api/shorts              ?mint&health&is_active
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

## Out of scope (v1)

- **Materialized candles** — until on-demand proves too slow.
- **Indexed image hosting (CDN/S3)** — `markets.image_url` stores the resolved URL; frontend fetches direct from arweave. Local cache layer is a follow-up if arweave outages become a problem.
- **Holder-distribution analytics** beyond count + top-N.
- **Cross-mint correlations / leaderboards.**
- **User-stats indexing** — current on-chain `UserStats` is bonding-only; the indexer's own `trades` and `swaps` aggregations will eventually supersede it, but for v1 we don't write a `user_stats` table.
