#!/usr/bin/env bash
# One-shot schema + role bootstrap against the fresh Cloud SQL instance.
# Cloud SQL doesn't run docker-entrypoint-initdb.d, so this applies what the
# local compose stack gets for free: 01-schema.sql + the TWO least-privilege
# roles (torch_ingest INSERT/UPDATE, torch_api SELECT-only).
#
# Idempotent: safe to re-run after a projection reset (roles survive DB
# drops; grants are re-applied).
#
# Prereqs: cloud-sql-proxy + psql; terraform applied (reads outputs).

set -euo pipefail
cd "$(dirname "$0")/terraform"

CONN=$(terraform output -raw sql_connection_name)
SUPER_PW=$(terraform output -raw pg_superuser_password)
INGEST_PW=$(terraform output -raw ingest_db_password)
API_PW=$(terraform output -raw api_db_password)

echo "starting cloud-sql-proxy for ${CONN}…"
cloud-sql-proxy --port 5433 "${CONN}" &
PROXY_PID=$!
trap 'kill ${PROXY_PID}' EXIT
sleep 3

export PGPASSWORD="${SUPER_PW}"
PSQL="psql -h 127.0.0.1 -p 5433 -U postgres -d torch -v ON_ERROR_STOP=1"

echo "applying schema…"
${PSQL} -f ../../indexer/db/01-schema.sql

echo "creating torch_ingest (INSERT/UPDATE)…"
${PSQL} <<EOSQL
    -- Idempotent: roles are INSTANCE-level and survive database drops, but
    -- their table grants die with the tables — re-running after a projection
    -- reset must re-grant without aborting on the existing role.
    DO \$\$ BEGIN
        CREATE ROLE torch_ingest LOGIN PASSWORD '${INGEST_PW}';
    EXCEPTION WHEN duplicate_object THEN
        RAISE NOTICE 'torch_ingest exists; re-granting';
    END \$\$;
    GRANT CONNECT ON DATABASE torch TO torch_ingest;
    GRANT USAGE ON SCHEMA public TO torch_ingest;
    GRANT SELECT, INSERT, UPDATE ON pools, reserves, swaps, liquidity_events TO torch_ingest;
    GRANT SELECT, INSERT, UPDATE ON markets, trades, messages, positions,
        position_events, migrations, indexer_state, metadata_fetch_log TO torch_ingest;
    GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO torch_ingest;
    REVOKE CREATE ON SCHEMA public FROM torch_ingest;
EOSQL

echo "creating torch_api (SELECT only)…"
${PSQL} <<EOSQL
    DO \$\$ BEGIN
        CREATE ROLE torch_api LOGIN PASSWORD '${API_PW}';
    EXCEPTION WHEN duplicate_object THEN
        RAISE NOTICE 'torch_api exists; re-granting';
    END \$\$;
    GRANT CONNECT ON DATABASE torch TO torch_api;
    GRANT USAGE ON SCHEMA public TO torch_api;
    GRANT SELECT ON pools, reserves, swaps, liquidity_events TO torch_api;
    GRANT SELECT ON markets, trades, messages, positions, position_events,
        migrations, indexer_state, metadata_fetch_log TO torch_api;
    REVOKE CREATE ON SCHEMA public FROM torch_api;
EOSQL

echo "bootstrap complete: schema + torch_ingest + torch_api"
