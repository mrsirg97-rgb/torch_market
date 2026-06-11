// [prompt-003] The pg_notify broadcast contract:
//   1. notifies are delivered ON COMMIT, with thin {"t","k"} payloads
//   2. a rolled-back write delivers NOTHING (the property that makes
//      DB-as-broadcaster correct)
//   3. backfill (no-checkpoint path notifies=false) stays silent
//   4. the torch_api role cannot write the projection (grant-level guarantee)

mod common;
use common::{fixtures::*, TestDb};

use sqlx::postgres::PgListener;
use torch_indexer::stream::writer::write_events_no_checkpoint;

async fn recv_with_timeout(listener: &mut PgListener, ms: u64) -> Option<(String, String)> {
    match tokio::time::timeout(std::time::Duration::from_millis(ms), listener.recv()).await {
        Ok(Ok(n)) => {
            let v: serde_json::Value = serde_json::from_str(n.payload()).unwrap();
            Some((
                v["t"].as_str().unwrap().to_string(),
                v["k"].as_str().unwrap().to_string(),
            ))
        }
        _ => None,
    }
}

#[tokio::test]
async fn commit_delivers_thin_notifies_rollback_delivers_nothing() {
    let db = TestDb::new().await;
    let mut listener = PgListener::connect_with(&db.pool).await.unwrap();
    listener.listen("torch_events").await.unwrap();

    // Rolled-back txn with a queued notify → silence.
    {
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_notify('torch_events', '{\"t\":\"trades\",\"k\":\"999\"}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        drop(tx); // rollback
    }
    assert!(
        recv_with_timeout(&mut listener, 300).await.is_none(),
        "rollback must deliver nothing"
    );

    // Committed txn with the same notify → delivered.
    {
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_notify('torch_events', '{\"t\":\"trades\",\"k\":\"1\"}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let n = recv_with_timeout(&mut listener, 1000).await;
    assert_eq!(n, Some(("trades".into(), "1".into())), "commit delivers");
}

#[tokio::test]
async fn backfill_path_does_not_notify() {
    let db = TestDb::new().await;
    let mut listener = PgListener::connect_with(&db.pool).await.unwrap();
    listener.listen("torch_events").await.unwrap();

    // write_events_no_checkpoint = the backfill path (notify suppressed).
    write_events_no_checkpoint(&db.pool, 100, vec![de(ev_market_created(1, 2), 100, 0)])
        .await
        .unwrap();

    assert!(
        recv_with_timeout(&mut listener, 300).await.is_none(),
        "backfill writes must not notify (catch-up would storm listeners)"
    );
}

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
