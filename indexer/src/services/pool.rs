// PoolService — composes pool-domain CRUD with the per-request cache.
// Ported verbatim from deep_pool/indexer.

use std::sync::Arc;

use crate::contracts::PoolRow;
use crate::domain::{pool, PoolFilter};
use crate::services::context::RequestCtx;

pub struct PoolService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> PoolService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn by_id(&mut self, pool_id: i32) -> sqlx::Result<Option<Arc<PoolRow>>> {
        if let Some(cached) = self.ctx.cache.pools_by_id.get(&pool_id) {
            return Ok(Some(Arc::clone(cached)));
        }
        let Some(row) = pool::get(&mut self.ctx.tx, pool_id).await? else {
            return Ok(None);
        };
        Ok(Some(self.cache_one(row)))
    }

    pub async fn by_pubkey(&mut self, pubkey: &str) -> sqlx::Result<Option<Arc<PoolRow>>> {
        if let Some(cached) = self.ctx.cache.pools_by_pubkey.get(pubkey) {
            return Ok(Some(Arc::clone(cached)));
        }
        let rows = pool::list(
            &mut self.ctx.tx,
            PoolFilter {
                pubkeys: Some(vec![pubkey.to_string()]),
                ..Default::default()
            },
        )
        .await?;
        Ok(rows.into_iter().next().map(|r| self.cache_one(r)))
    }

    pub async fn for_token_mints(
        &mut self,
        token_mints: Vec<String>,
    ) -> sqlx::Result<Vec<Arc<PoolRow>>> {
        let rows = pool::list(
            &mut self.ctx.tx,
            PoolFilter {
                token_mints: Some(token_mints),
                ..Default::default()
            },
        )
        .await?;
        Ok(self.cache_many(rows))
    }

    pub async fn for_creators(&mut self, creators: Vec<String>) -> sqlx::Result<Vec<Arc<PoolRow>>> {
        let rows = pool::list(
            &mut self.ctx.tx,
            PoolFilter {
                creators: Some(creators),
                ..Default::default()
            },
        )
        .await?;
        Ok(self.cache_many(rows))
    }

    pub async fn list(&mut self, filter: PoolFilter) -> sqlx::Result<Vec<Arc<PoolRow>>> {
        let rows = pool::list(&mut self.ctx.tx, filter).await?;
        Ok(self.cache_many(rows))
    }

    fn cache_one(&mut self, row: PoolRow) -> Arc<PoolRow> {
        let arc = Arc::new(row);
        self.ctx
            .cache
            .pools_by_id
            .insert(arc.pool_id, Arc::clone(&arc));
        self.ctx
            .cache
            .pools_by_pubkey
            .insert(arc.pubkey.clone(), Arc::clone(&arc));
        arc
    }

    fn cache_many(&mut self, rows: Vec<PoolRow>) -> Vec<Arc<PoolRow>> {
        rows.into_iter().map(|r| self.cache_one(r)).collect()
    }
}
