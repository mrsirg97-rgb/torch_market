// The block notify contract behind the generated read tier's keep-fresh:
//   1. a live write delivers ONE thin notify on COMMIT, naming the slot and
//      the tables the block wrote
//   2. a rolled-back write delivers NOTHING
//   3. backfill (the no-checkpoint path) stays silent

mod common;
use common::{fixtures::*, TestDb};

use sqlx::postgres::PgListener;
use torch_indexer::stream::writer::{write_events_no_checkpoint, write_events_notify};

async fn recv_with_timeout(listener: &mut PgListener, ms: u64) -> Option<serde_json::Value> {
    match tokio::time::timeout(std::time::Duration::from_millis(ms), listener.recv()).await {
        Ok(Ok(n)) => Some(serde_json::from_str(n.payload()).unwrap()),
        _ => None,
    }
}

#[tokio::test]
async fn live_write_delivers_one_block_notify_on_commit() {
    let db = TestDb::new().await;
    let mut listener = PgListener::connect_with(&db.pool).await.unwrap();
    listener.listen("torch_events").await.unwrap();

    write_events_notify(&db.pool, 100, vec![de(ev_market_created(1, 2), 100, 0)])
        .await
        .unwrap();

    let n = recv_with_timeout(&mut listener, 1000)
        .await
        .expect("a live write notifies on commit");
    assert_eq!(n["slot"], 100);
    let tables: Vec<&str> = n["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(tables.contains(&"markets"), "payload names the written table: {n}");
    assert!(
        recv_with_timeout(&mut listener, 300).await.is_none(),
        "one block, one notify"
    );
}

#[tokio::test]
async fn rollback_delivers_nothing() {
    let db = TestDb::new().await;
    let mut listener = PgListener::connect_with(&db.pool).await.unwrap();
    listener.listen("torch_events").await.unwrap();

    {
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_notify('torch_events', '{\"slot\":1,\"tables\":[\"trades\"]}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        drop(tx);
    }
    assert!(
        recv_with_timeout(&mut listener, 300).await.is_none(),
        "rollback must deliver nothing"
    );
}

#[tokio::test]
async fn backfill_path_does_not_notify() {
    let db = TestDb::new().await;
    let mut listener = PgListener::connect_with(&db.pool).await.unwrap();
    listener.listen("torch_events").await.unwrap();

    write_events_no_checkpoint(&db.pool, 100, vec![de(ev_market_created(1, 2), 100, 0)])
        .await
        .unwrap();

    assert!(
        recv_with_timeout(&mut listener, 300).await.is_none(),
        "backfill writes must not notify (catch-up would storm listeners)"
    );
}
