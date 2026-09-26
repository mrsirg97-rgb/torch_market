// The grant-level guarantee behind the read/write split: the torch_api role
// can read the projection and can never mutate it. Enforced by GRANT, not
// convention (db/03-api-role.sh); this pins it against an ephemeral DB.

mod common;
use common::{fixtures::*, TestDb};

use torch_indexer::stream::writer::write_events_no_checkpoint;

#[tokio::test]
async fn api_role_cannot_write_the_projection() {
    let db = TestDb::new().await;
    // Create the SELECT-only role in this ephemeral DB (mirrors
    // db/03-api-role.sh) and connect as it.
    sqlx::query("CREATE ROLE torch_api_t LOGIN PASSWORD 'pw'")
        .execute(&db.pool)
        .await
        .ok(); // role may exist from a parallel test DB — it's cluster-global
    for q in [
        "GRANT CONNECT ON DATABASE {db} TO torch_api_t",
        "GRANT USAGE ON SCHEMA public TO torch_api_t",
        "GRANT SELECT ON markets, trades TO torch_api_t",
    ] {
        sqlx::query(&q.replace("{db}", &db.name))
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let api_url = db.url_for_user("torch_api_t", "pw");
    let api_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&api_url)
        .await
        .unwrap();

    // Seed one market via the WRITER (superuser pool) …
    write_events_no_checkpoint(&db.pool, 100, vec![de(ev_market_created(1, 2), 100, 0)])
        .await
        .unwrap();

    // … the api role can read it …
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM markets")
        .fetch_one(&api_pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // … but every mutation is rejected by grant.
    for stmt in [
        "INSERT INTO trades (mint, trader, is_buy, sol_in, sol_out, tokens_in, tokens_out, sol_to_treasury, sol_to_creator, protocol_fee, virtual_sol_after, virtual_token_after, real_sol_after, real_token_after, slot, signature, inner_ix_idx, created_at) VALUES ('x','y',true,0,0,0,0,0,0,0,0,0,0,0,1,'s',0,NOW())",
        "UPDATE markets SET real_sol = 999",
        "DELETE FROM markets",
    ] {
        let err = sqlx::query(stmt).execute(&api_pool).await;
        assert!(err.is_err(), "api role must not be able to: {stmt}");
        let msg = format!("{:?}", err.unwrap_err());
        assert!(
            msg.contains("permission denied"),
            "expected permission denied, got: {msg}"
        );
    }
}
