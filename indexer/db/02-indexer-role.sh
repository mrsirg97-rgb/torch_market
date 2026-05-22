#!/bin/bash
# Runs after 01-schema.sql (Postgres init scripts execute in lexical order).
# Creates the indexer's least-privilege role and grants exactly the rights
# the indexer needs — nothing more.
#
# Runs as $POSTGRES_USER (superuser) against $POSTGRES_DB.
# INDEXER_DB_PASSWORD is supplied via docker-compose.yml env.

set -euo pipefail

if [[ -z "${INDEXER_DB_PASSWORD:-}" ]]; then
    echo "ERROR: INDEXER_DB_PASSWORD env var not set. Cannot create indexer role." >&2
    exit 1
fi

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    -- Least-privilege role. Only LOGIN + what's granted below; no CREATE,
    -- no DELETE, no TRUNCATE, no access to schemas beyond public.
    CREATE ROLE torch_indexer LOGIN PASSWORD '${INDEXER_DB_PASSWORD}';

    -- Connect + schema usage
    GRANT CONNECT ON DATABASE ${POSTGRES_DB} TO torch_indexer;
    GRANT USAGE  ON SCHEMA   public          TO torch_indexer;

    -- Deep_pool event tables (this indexer also ingests deep_pool events).
    GRANT SELECT, INSERT, UPDATE ON pools             TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON reserves          TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON swaps             TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON liquidity_events  TO torch_indexer;

    -- Torch market tables. INSERT for the writer; UPDATE for upsert paths
    -- on markets / loans / shorts (current-state snapshots, not append-only).
    GRANT SELECT, INSERT, UPDATE ON markets    TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON trades     TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON messages   TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON loans      TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON shorts     TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON migrations TO torch_indexer;

    -- Cross-cutting tables.
    GRANT SELECT, INSERT, UPDATE ON indexer_state      TO torch_indexer;
    GRANT SELECT, INSERT, UPDATE ON metadata_fetch_log TO torch_indexer;

    -- SERIAL columns generate sequences. INSERT on a table doesn't implicitly
    -- grant nextval() — must grant USAGE on the sequence too. ALL SEQUENCES
    -- covers every event table's id sequence.
    GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO torch_indexer;

    -- Explicitly deny schema-modifying rights. (Already implicit from not
    -- granting, but stating it makes the security model auditable.)
    REVOKE CREATE ON SCHEMA public FROM torch_indexer;
EOSQL

echo "created torch_indexer role with least-privilege grants"
