#!/bin/bash
# Runs after 02-indexer-role.sh (lexical order). Creates the read API's
# SELECT-only role (prompt-003 split). The API service can never mutate the
# projection — the DB is a projection of chain state and only the single
# ingest writer projects it. Separation enforced by GRANT, not convention.
#
# LISTEN requires no table grants; pg_notify fires from the ingest role's
# write transactions and Postgres delivers to any listening session.

set -euo pipefail

if [[ -z "${API_DB_PASSWORD:-}" ]]; then
    echo "ERROR: API_DB_PASSWORD env var not set. Cannot create api role." >&2
    exit 1
fi

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE ROLE torch_api LOGIN PASSWORD '${API_DB_PASSWORD}';

    GRANT CONNECT ON DATABASE ${POSTGRES_DB} TO torch_api;
    GRANT USAGE  ON SCHEMA   public          TO torch_api;

    -- SELECT only, on everything the API serves. No INSERT, no UPDATE, no
    -- DELETE, no TRUNCATE, no sequences, nothing future-granted by default.
    GRANT SELECT ON pools, reserves, swaps, liquidity_events TO torch_api;
    GRANT SELECT ON markets, trades, messages, positions, position_events,
                    migrations, indexer_state, metadata_fetch_log TO torch_api;

    REVOKE CREATE ON SCHEMA public FROM torch_api;
EOSQL

echo "created torch_api role (SELECT-only)"
