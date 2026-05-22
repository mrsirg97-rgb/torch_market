// Integration-test harness.
//
// Each test gets a uniquely-named ephemeral Postgres database created from
// the canonical `db/01-schema.sql`. The database is dropped on `TestDb::drop`
// so tests can run in parallel without stepping on each other and a fresh
// suite leaves no residue.
//
// Requires `TEST_DATABASE_URL` to point at a Postgres instance with CREATE
// DATABASE privileges. Easiest path: `docker compose up postgres` then
//   export TEST_DATABASE_URL=postgres://torch:<POSTGRES_PASSWORD>@127.0.0.1:5432/torch
// (the docker-compose Postgres user is the superuser; the indexer's limited
// role can't create databases and shouldn't be used for tests).
//
// Test panics leak databases (cleanup is in Drop, which doesn't run on
// abort). Periodically purge with:
//   SELECT 'DROP DATABASE "' || datname || '"'
//   FROM pg_database WHERE datname LIKE 'torch_test_%';

#![allow(dead_code)]

use std::str::FromStr;

use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, PgPool};

pub mod fixtures;

const TEST_DB_URL_VAR: &str = "TEST_DATABASE_URL";

pub struct TestDb {
    pub pool: PgPool,
    db_name: String,
    admin_url: String,
}

impl TestDb {
    pub async fn new() -> Self {
        let admin_url = std::env::var(TEST_DB_URL_VAR).unwrap_or_else(|_| {
            panic!(
                "{TEST_DB_URL_VAR} not set. Start postgres via `docker compose up postgres` \
                 and export e.g. \
                 TEST_DATABASE_URL=postgres://torch:<password>@127.0.0.1:5432/torch"
            )
        });

        let db_name = format!("torch_test_{}", uuid::Uuid::new_v4().simple());

        // Create the test DB against the admin connection.
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect to admin DB");
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create test DB");
        admin.close().await;

        // Connect to the freshly-created DB and apply the schema.
        let opts = PgConnectOptions::from_str(&admin_url)
            .expect("parse admin url")
            .database(&db_name)
            // Silence sqlx's per-statement INFO logs during schema apply.
            .log_statements(tracing::log::LevelFilter::Off);
        let pool = PgPool::connect_with(opts)
            .await
            .expect("connect to test DB");

        let schema = include_str!("../../db/01-schema.sql");
        sqlx::raw_sql(schema)
            .execute(&pool)
            .await
            .expect("apply schema");

        Self {
            pool,
            db_name,
            admin_url,
        }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Async cleanup from sync Drop: spawn a blocking helper thread that
        // owns its own tokio runtime, drop the test DB, then join. Joining
        // ensures the DROP completes before the test process exits (which
        // would otherwise leak DBs on fast test suites).
        let admin_url = self.admin_url.clone();
        let db_name = self.db_name.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            rt.block_on(async move {
                if let Ok(admin) = PgPool::connect(&admin_url).await {
                    // FORCE terminates lingering connections to the test DB
                    // (the test's PgPool may not be fully dropped yet).
                    let _ = sqlx::query(&format!(
                        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
                    ))
                    .execute(&admin)
                    .await;
                }
            });
        })
        .join();
    }
}

// Convenience: load the schema string at compile time so individual test
// files can call this without re-reading the file. Same content as
// TestDb::new()'s apply step.
pub fn schema_sql() -> &'static str {
    include_str!("../../db/01-schema.sql")
}
