// Per-request transaction + memo cache. Direct copy of the pattern in
// deep_pool/indexer/src/services/context.rs.
//
// Cache covers entities that are looked up multiple times per request:
// pools (resolved by pool_id and pubkey), latest reserves per pool, and
// markets (by mint). Append-only event rows (trades, swaps, liquidity,
// messages, loans, shorts, migrations) are not cached — they're large,
// list-ordered, and rarely re-queried within a single request.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::{PgPool, Postgres, Transaction};

use crate::contracts::{MarketRow, PoolRow, ReservesRow};

#[derive(Default)]
pub struct Cache {
    pub pools_by_id: HashMap<i32, Arc<PoolRow>>,
    pub pools_by_pubkey: HashMap<String, Arc<PoolRow>>,
    pub reserves_by_pool_id: HashMap<i32, Arc<ReservesRow>>,
    pub markets_by_mint: HashMap<String, Arc<MarketRow>>,
}

pub struct RequestCtx {
    pub tx: Transaction<'static, Postgres>,
    pub cache: Cache,
}

impl RequestCtx {
    // Open a transaction at REPEATABLE READ for snapshot consistency across
    // every query in the request. Postgres requires SET TRANSACTION ISOLATION
    // to be the first statement after BEGIN; sqlx's `pool.begin()` issues
    // BEGIN, so this is the next thing we run.
    pub async fn begin(pool: &PgPool) -> sqlx::Result<Self> {
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        Ok(Self {
            tx,
            cache: Cache::default(),
        })
    }

    pub async fn commit(self) -> sqlx::Result<()> {
        self.tx.commit().await
    }

    pub async fn rollback(self) -> sqlx::Result<()> {
        self.tx.rollback().await
    }
}
