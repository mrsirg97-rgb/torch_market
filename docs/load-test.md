# Load test — read API (prompt-003 split)

Validates the split read service before the public preview: p50–p99 under
query concurrency, plus WS room fan-out latency through the pg_notify bridge.
Harness: `/loadtest` (tokio-native, no external tooling); seeder:
`indexer loadseed` (drives the REAL writer path so the API serves
production-shaped rows).

## Results (2026-06-11, local: M-series laptop, release builds, compose Postgres)

Seed: 50 markets, ~4,000 trades (half on the hot mint), 50 shorts.

### 64 concurrent (the preview-gate run)

| endpoint | reqs | rps | p50 ms | p90 ms | p95 ms | p99 ms | max ms | non-200 | 5xx |
|---|---|---|---|---|---|---|---|---|---|
| markets_list | 19948 | 2494 | 25.5 | 28.8 | 30.3 | 33.9 | 50.7 | 0 | 0 |
| market_detail | 21541 | 2693 | 23.7 | 26.4 | 27.5 | 31.1 | 35.2 | 0 | 0 |
| trades_page | 17262 | 2158 | 29.4 | 32.5 | 33.9 | 37.6 | 43.7 | 0 | 0 |
| candles_1m | 10554 | 1319 | 48.6 | 52.4 | 53.7 | 56.2 | 61.3 | 0 | 0 |
| positions | 21394 | 2674 | 23.9 | 26.7 | 27.8 | 30.9 | 34.4 | 0 | 0 |
| messages | 20891 | 2611 | 24.4 | 27.4 | 28.3 | 30.7 | 35.2 | 0 | 0 |

**Gates: PASSED with ~8× headroom.** (list/detail p99 < 250ms → measured ≤34ms;
candles p99 < 500ms → measured 56ms; zero 5xx.)

### 256 concurrent (headroom probe)

| endpoint | rps | p50 ms | p99 ms | non-200 |
|---|---|---|---|---|
| markets_list | 2491 | 103.3 | 117.8 | 0 |
| market_detail | 2697 | 96.1 | 107.0 | 0 |
| trades_page | 2296 | 112.4 | 124.6 | 0 |
| candles_1m | 1179 | 222.8 | 235.3 | 0 |
| positions | 2688 | 95.9 | 108.3 | 0 |
| messages | 2591 | 99.8 | 110.9 | 0 |

Throughput is FLAT from 64 → 256 concurrency (≈2.5k rps/endpoint) while
latency scales ~linearly — the signature of saturated capacity with queueing,
not collapse: requests wait, none fail. Single-instance ceiling ≈ 2.5–2.7k rps
per endpoint class; candles ≈ 1.2k (the SQL aggregation, as predicted —
first candidate for a materialized view if it ever matters, per indexer.md
decision #6 it stays on-demand until measured need).

### WS room fan-out (50 clients, one market room, ~20 notifying writes/s)

```
frames=17,100 | delivery p50=8.9ms p95=11.1ms p99=12.5ms max=22.4ms
```

Delivery = writer txn COMMIT → pg_notify → API row fetch → room broadcast →
client receive (timestamp embedded in memo_text by `loadseed --notify`).
**Sub-13ms p99 end-to-end through the whole split.**

## Gotcha worth remembering

The seeder originally reused identical synthetic signatures across runs —
every row hit the `(signature, inner_ix_idx)` idempotency key, inserted
nothing, and a zero-row block queues ZERO notifies. First run worked, every
re-run was a silent no-op. The idempotency invariant applies to load tooling
too: signatures are now nonced per run.

## How to run

```bash
# one-time: load DB + seed
docker exec torch-indexer-pg psql -U torch -c "CREATE DATABASE torch_load"
docker exec -i torch-indexer-pg psql -U torch -d torch_load < indexer/db/01-schema.sql
DATABASE_URL=postgres://torch:<pw>@127.0.0.1:5432/torch_load \
  cargo run --manifest-path indexer/Cargo.toml --release --bin loadseed -- \
  --markets 50 --trades 4000 --positions 50          # prints the hot mint

# serve
DATABASE_URL=... API_BIND=127.0.0.1:8081 ./api/target/release/torch-api

# HTTP percentiles
cargo run --manifest-path loadtest/Cargo.toml --release -- \
  --url http://127.0.0.1:8081 --concurrency 64 --duration 10 --mint <HOT_MINT>

# WS fan-out (run loadseed --notify concurrently)
DATABASE_URL=... cargo run --manifest-path indexer/Cargo.toml --release \
  --bin loadseed -- --notify --rate 20 --seconds 12 &
cargo run --manifest-path loadtest/Cargo.toml --release -- \
  --url http://127.0.0.1:8081 --concurrency 50 --duration 14 --mint <HOT_MINT> --ws-only
```

Re-measure on Cloud Run during the GCP rollout (prompt-004) for
local-vs-cloud deltas before opening the preview.
