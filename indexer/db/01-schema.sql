-- Torch indexer schema.
-- Single Postgres database, two program streams (torch_market + deep_pool).
-- See docs/indexer.md for design rationale.
--
-- Conventions:
--   - All event tables carry UNIQUE (signature, inner_ix_idx) so backfill
--     can replay without dupes.
--   - Position tables are CURRENT snapshots, not event logs — closed
--     positions stay as rows with is_active=false (audit trail).
--   - Token-2022 fee semantics: amounts stored are NET (what the
--     recipient actually received), matching the on-chain
--     tokens_borrowed = net fix. Gross is never stored.

-- ============================================================================
-- Custom enum types
-- ============================================================================

DO $$ BEGIN
    CREATE TYPE market_status AS ENUM ('BONDING', 'COMPLETE', 'MIGRATED', 'RECLAIMED');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
    CREATE TYPE market_tier AS ENUM ('flame', 'torch');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
    CREATE TYPE position_health AS ENUM ('healthy', 'at_risk', 'liquidatable', 'none');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- [V21] Unified leverage: one Position struct on-chain, side = long | short.
DO $$ BEGIN
    CREATE TYPE position_side AS ENUM ('long', 'short');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
    CREATE TYPE position_event_kind AS ENUM ('open', 'close', 'liquidate');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
    CREATE TYPE metadata_status AS ENUM ('ok', 'not_found', 'error');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- ============================================================================
-- DEEP_POOL TABLES (verbatim from deep_pool/db/01-schema.sql)
--
-- Same indexer process ingests both program streams; deep_pool tables live
-- here unchanged so the existing deeppoolsdk binary contract is preserved.
-- ============================================================================

CREATE TABLE IF NOT EXISTS pools (
    pool_id            SERIAL PRIMARY KEY,
    pubkey             TEXT NOT NULL UNIQUE,
    config             TEXT NOT NULL,
    token_mint         TEXT NOT NULL,
    lp_mint            TEXT NOT NULL,
    creator            TEXT NOT NULL,
    sol_initial        BIGINT NOT NULL,
    tokens_initial     BIGINT NOT NULL,
    lp_supply_initial  BIGINT NOT NULL,
    slot               BIGINT NOT NULL,
    signature          TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS pools_token_mint_idx ON pools(token_mint);
CREATE INDEX IF NOT EXISTS pools_creator_idx    ON pools(creator);

CREATE TABLE IF NOT EXISTS reserves (
    reserve_id     SERIAL PRIMARY KEY,
    pool_id        INT NOT NULL REFERENCES pools(pool_id),
    sol_reserve    BIGINT NOT NULL,
    token_reserve  BIGINT NOT NULL,
    lp_supply      BIGINT NOT NULL,
    last_slot      BIGINT NOT NULL,
    signature      TEXT NOT NULL,
    inner_ix_idx   INT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS reserves_pool_slot_idx
    ON reserves(pool_id, last_slot DESC, signature DESC, inner_ix_idx DESC);

CREATE TABLE IF NOT EXISTS swaps (
    swap_id              SERIAL PRIMARY KEY,
    pool_id              INT NOT NULL REFERENCES pools(pool_id),
    user_pk              TEXT NOT NULL,
    sol_source           TEXT NOT NULL,
    is_buy               BOOLEAN NOT NULL,
    amount_in_gross      BIGINT NOT NULL,
    amount_in_net        BIGINT NOT NULL,
    amount_out_gross     BIGINT NOT NULL,
    amount_out_net       BIGINT NOT NULL,
    fee                  BIGINT NOT NULL,
    sol_reserve_after    BIGINT NOT NULL,
    token_reserve_after  BIGINT NOT NULL,
    slot                 BIGINT NOT NULL,
    signature            TEXT NOT NULL,
    inner_ix_idx         INT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS swaps_pool_created_idx ON swaps(pool_id, created_at DESC);
CREATE INDEX IF NOT EXISTS swaps_user_created_idx ON swaps(user_pk, created_at DESC);

CREATE TABLE IF NOT EXISTS liquidity_events (
    liquidity_id         SERIAL PRIMARY KEY,
    pool_id              INT NOT NULL REFERENCES pools(pool_id),
    provider             TEXT NOT NULL,
    is_add               BOOLEAN NOT NULL,
    sol_amount_gross     BIGINT NOT NULL,
    sol_amount_net       BIGINT NOT NULL,
    tokens_amount_gross  BIGINT NOT NULL,
    tokens_amount_net    BIGINT NOT NULL,
    lp_user_amount       BIGINT NOT NULL,
    lp_locked            BIGINT NOT NULL,
    lp_supply_after      BIGINT NOT NULL,
    slot                 BIGINT NOT NULL,
    signature            TEXT NOT NULL,
    inner_ix_idx         INT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS liquidity_pool_created_idx     ON liquidity_events(pool_id, created_at DESC);
CREATE INDEX IF NOT EXISTS liquidity_provider_created_idx ON liquidity_events(provider, created_at DESC);

-- ============================================================================
-- TORCH TABLES
-- ============================================================================

-- One row per mint. Current state; mutated by every event that changes
-- bonding-curve reserves, status, or migration.
CREATE TABLE IF NOT EXISTS markets (
    mint                    TEXT PRIMARY KEY,
    name                    TEXT NOT NULL,
    symbol                  TEXT NOT NULL,
    metadata_uri            TEXT,
    image_url               TEXT,
    creator                 TEXT NOT NULL,
    is_community_token      BOOLEAN NOT NULL,
    status                  market_status NOT NULL,
    tier                    market_tier   NOT NULL,
    sol_target              BIGINT NOT NULL,

    -- Bonding curve state (updated on every Buy/Sell)
    virtual_sol             BIGINT NOT NULL,
    virtual_token           BIGINT NOT NULL,
    real_sol                BIGINT NOT NULL,
    real_token              BIGINT NOT NULL,

    -- Lifecycle slots
    created_at_slot         BIGINT NOT NULL,
    bonding_complete_slot   BIGINT,
    migrated_slot           BIGINT,
    reclaimed_slot          BIGINT,
    last_activity_slot      BIGINT NOT NULL,

    -- Post-migration link (NULL until migrate_to_dex commits)
    deep_pool_pubkey        TEXT REFERENCES pools(pubkey),

    created_at              TIMESTAMPTZ NOT NULL,
    updated_at              TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS markets_creator_idx           ON markets(creator, created_at DESC);
CREATE INDEX IF NOT EXISTS markets_status_real_sol_idx   ON markets(status, real_sol DESC);
CREATE INDEX IF NOT EXISTS markets_status_activity_idx   ON markets(status, last_activity_slot DESC);
CREATE INDEX IF NOT EXISTS markets_deep_pool_idx         ON markets(deep_pool_pubkey) WHERE deep_pool_pubkey IS NOT NULL;

-- Bonding curve trade events (Buy, BuyViaVault, Sell, SellViaVault).
-- One row per inner ix. tokens_out is NET (post-Token-2022-fee).
-- See docs/indexer.md §"Token-2022 net semantics".
CREATE TABLE IF NOT EXISTS trades (
    trade_id                SERIAL PRIMARY KEY,
    mint                    TEXT NOT NULL REFERENCES markets(mint),
    trader                  TEXT NOT NULL,
    -- Vault PDA if the trade routed through a vault, NULL for direct trades.
    vault                   TEXT,
    is_buy                  BOOLEAN NOT NULL,
    sol_in                  BIGINT NOT NULL,  -- on buys: SOL paid; on sells: 0
    sol_out                 BIGINT NOT NULL,  -- on sells: SOL received; on buys: 0
    tokens_in               BIGINT NOT NULL,  -- on sells: tokens sent (net); on buys: 0
    tokens_out              BIGINT NOT NULL,  -- on buys:  tokens received (net); on sells: 0
    sol_to_treasury         BIGINT NOT NULL,
    sol_to_creator          BIGINT NOT NULL,
    protocol_fee            BIGINT NOT NULL,
    -- Curve state immediately after the trade (snapshotted for chart paint)
    virtual_sol_after       BIGINT NOT NULL,
    virtual_token_after     BIGINT NOT NULL,
    real_sol_after          BIGINT NOT NULL,
    real_token_after        BIGINT NOT NULL,
    slot                    BIGINT NOT NULL,
    signature               TEXT NOT NULL,
    inner_ix_idx            INT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS trades_mint_slot_idx
    ON trades(mint, slot DESC, signature DESC, inner_ix_idx DESC);
CREATE INDEX IF NOT EXISTS trades_trader_slot_idx
    ON trades(trader, slot DESC);

-- Memo-program messages attached to a torch instruction in the same tx.
-- Gated by stream/decoder.rs: only persisted when the tx also contains a
-- torch_market ix; otherwise dropped. Per-market message board (not per-user inbox).
CREATE TABLE IF NOT EXISTS messages (
    message_id              SERIAL PRIMARY KEY,
    mint                    TEXT NOT NULL REFERENCES markets(mint),
    sender                  TEXT NOT NULL,
    memo_text               TEXT NOT NULL,
    -- Action that the memo accompanied: 'buy' | 'sell' | 'open_short' | 'close_short' | ...
    -- Free-form; the API doesn't filter on it but the UI uses it for display chrome.
    action_kind             TEXT,
    slot                    BIGINT NOT NULL,
    signature               TEXT NOT NULL,
    inner_ix_idx            INT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS messages_mint_slot_idx     ON messages(mint, slot DESC);
CREATE INDEX IF NOT EXISTS messages_sender_slot_idx   ON messages(sender, slot DESC);

-- [V21] Unified leverage positions. One Position struct on-chain (side = long |
-- short); a user can hold many per (mint, owner) distinguished by position_index.
-- Current-state, UPSERTed on every open/close/liquidate; is_active=false marks the
-- row historical (we never DELETE — the row is the audit trail). `owner` may be a
-- wallet or a torch_vault PDA (owner_is_vault). `collateral_amount`/`debt_amount`
-- are unit-by-side (mirrors the on-chain generic fields):
--   long  → collateral = tokens,   debt = lamports, vault_balance = tokens
--   short → collateral = lamports,  debt = tokens,   vault_balance = lamports
CREATE TABLE IF NOT EXISTS positions (
    mint                        TEXT NOT NULL REFERENCES markets(mint),
    owner                       TEXT NOT NULL,
    side                        position_side NOT NULL,
    position_index              INTEGER NOT NULL,
    collateral_amount           BIGINT NOT NULL,
    debt_amount                 BIGINT NOT NULL,
    open_fee_sol                BIGINT NOT NULL,
    vault_balance               BIGINT NOT NULL,
    accrued_interest_stored     BIGINT NOT NULL,
    last_update_slot            BIGINT NOT NULL,
    -- Snapshot at last write. API recomputes live health against the deep_pool TWAP mark.
    health                      position_health NOT NULL,
    is_active                   BOOLEAN NOT NULL,
    owner_is_vault              BOOLEAN NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (mint, owner, side, position_index)
);

CREATE INDEX IF NOT EXISTS positions_mint_active_idx
    ON positions(mint, side, health, updated_at DESC) WHERE is_active = true;
CREATE INDEX IF NOT EXISTS positions_owner_idx
    ON positions(owner, updated_at DESC) WHERE is_active = true;

-- [V21] Append-only leverage event log (the analog of `trades`). Preserves the
-- per-event data — especially liquidation analytics (bad_debt, twap_ltv, bonus_bps,
-- seized) — that the current-state `positions` table can't retain. NULL columns are
-- kind-specific (e.g. liquidation fields are NULL on open/close).
CREATE TABLE IF NOT EXISTS position_events (
    -- [I-1] serial = intra-slot chain order (single chain-ordered writer)
    event_id                    BIGSERIAL PRIMARY KEY,
    mint                        TEXT NOT NULL REFERENCES markets(mint),
    owner                       TEXT NOT NULL,
    side                        position_side NOT NULL,
    position_index              INTEGER NOT NULL,
    kind                        position_event_kind NOT NULL,
    liquidator                  TEXT,            -- liquidate only
    sol_in                      BIGINT,          -- open/close legs
    sol_out                     BIGINT,
    tokens_in                   BIGINT,
    tokens_out                  BIGINT,
    interest_paid               BIGINT,          -- close / liquidate
    principal_paid              BIGINT,
    surplus_sol                 BIGINT,          -- close
    bad_debt                    BIGINT,          -- liquidate
    twap_ltv                    BIGINT,
    bonus_bps                   INTEGER,
    seized                      BIGINT,          -- sol_seized (short) / tokens_seized (long)
    residual                    BIGINT,          -- residual to borrower (short)
    fully_resolved              BOOLEAN,         -- fully_closed / fully_liquidated
    slot                        BIGINT NOT NULL,
    signature                   TEXT NOT NULL,
    inner_ix_idx                INTEGER NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL,
    UNIQUE (signature, inner_ix_idx)
);

CREATE INDEX IF NOT EXISTS position_events_mint_slot_idx  ON position_events(mint, slot DESC);
CREATE INDEX IF NOT EXISTS position_events_owner_slot_idx ON position_events(owner, slot DESC);
CREATE INDEX IF NOT EXISTS position_events_kind_slot_idx  ON position_events(kind, slot DESC);

-- Migration events (bonding curve → DeepPool). One row per mint.
-- Written atomically with the corresponding `pools` row insert.
CREATE TABLE IF NOT EXISTS migrations (
    mint                    TEXT PRIMARY KEY REFERENCES markets(mint),
    deep_pool_pubkey        TEXT NOT NULL REFERENCES pools(pubkey),
    sol_seeded              BIGINT NOT NULL,
    tokens_seeded           BIGINT NOT NULL,
    lp_burned               BIGINT NOT NULL,
    slot                    BIGINT NOT NULL,
    signature               TEXT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS migrations_pool_idx ON migrations(deep_pool_pubkey);

-- ============================================================================
-- CROSS-CUTTING TABLES
-- ============================================================================

-- Single-row table tracking how far the indexer has caught up. Yellowstone
-- is slot-ordered globally across all subscribed programs, so one cursor
-- covers both torch_market and deep_pool ingestion.
CREATE TABLE IF NOT EXISTS indexer_state (
    id INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    last_processed_slot BIGINT NOT NULL
);

-- Tracks off-chain metadata fetches (Arweave/Irys URIs). Avoids re-fetching
-- dead URIs on every restart; populated by an async sidecar task that scans
-- markets.metadata_uri rows lacking a successful entry here.
CREATE TABLE IF NOT EXISTS metadata_fetch_log (
    mint                TEXT PRIMARY KEY REFERENCES markets(mint),
    metadata_uri        TEXT NOT NULL,
    status              metadata_status NOT NULL,
    image_url           TEXT,      -- mirrored into markets.image_url on success
    twitter_url         TEXT,
    telegram_url        TEXT,
    website_url         TEXT,
    description         TEXT,
    last_fetched_at     TIMESTAMPTZ NOT NULL,
    fetch_count         INT NOT NULL DEFAULT 1,
    last_error          TEXT
);

CREATE INDEX IF NOT EXISTS metadata_fetch_status_idx ON metadata_fetch_log(status, last_fetched_at);
