# torch-indexer tests

Two tiers, separable via `cargo test --test <name>`.

## Tier 1 — pure-function tests (no DB)

```bash
cargo test --test decoder_test --test discriminator_test --test translate_test
```

31 tests covering:
- `decoder_test.rs` — Borsh roundtrip + error paths (TooShort, UnknownDiscriminator, TrailingBytes) + cross-program isolation.
- `discriminator_test.rs` — pins all 16 event discriminators against `shasum`-computed ground truth + asserts no collisions.
- `translate_test.rs` — event → `New*Row` mapping (pubkey b58, vault sentinel → None, tier-from-target boundaries, net-amount semantics).

Runs in milliseconds. No infrastructure required.

## Tier 2 — integration tests (Postgres required)

40 tests across `it_domain_test.rs`, `it_writer_test.rs`, `it_api_test.rs`. Cover DB CRUD, FK constraints, three-phase writer ordering, idempotent replay, memo gating, position-delta application, and HTTP endpoint integration (via `tower::ServiceExt::oneshot`).

### Setup

```bash
# 1. Start postgres
docker compose up -d postgres

# 2. Point tests at it. Use the SUPERUSER role (not the limited
#    torch_indexer role) — tests create+drop ephemeral databases.
export TEST_DATABASE_URL="postgres://torch:$POSTGRES_PASSWORD@127.0.0.1:5432/torch"

# 3. Run
cargo test --test it_domain_test --test it_writer_test --test it_api_test
```

Each test creates a uniquely-named database (`torch_test_<uuid>`) on entry and drops it on exit. Tests run in parallel.

### Run everything

```bash
cargo test
```

Tier 2 panics with a clear `TEST_DATABASE_URL not set` message if the env var is missing, so the failure mode is unambiguous in CI.

### Cleanup leaked databases

Test panics abort the process before `Drop` cleanup, so a leaked DB may remain. Purge with:

```sql
-- Run as superuser against the admin DB
SELECT 'DROP DATABASE "' || datname || '" WITH (FORCE);'
FROM pg_database
WHERE datname LIKE 'torch_test_%';
```

Copy the output, execute it, done.
