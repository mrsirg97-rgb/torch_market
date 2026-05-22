// ReservesService — "current" reserves per pool with caching, plus history.
// Ported verbatim from deep_pool/indexer.

use std::collections::HashMap;
use std::sync::Arc;

use crate::contracts::ReservesRow;
use crate::domain::{reserves, ReservesFilter};
use crate::services::context::RequestCtx;

pub struct ReservesService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> ReservesService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn latest_for_pool(
        &mut self,
        pool_id: i32,
    ) -> sqlx::Result<Option<Arc<ReservesRow>>> {
        if let Some(cached) = self.ctx.cache.reserves_by_pool_id.get(&pool_id) {
            return Ok(Some(Arc::clone(cached)));
        }
        let mut map = self.latest_for_pools(vec![pool_id]).await?;
        Ok(map.remove(&pool_id))
    }

    pub async fn latest_for_pools(
        &mut self,
        pool_ids: Vec<i32>,
    ) -> sqlx::Result<HashMap<i32, Arc<ReservesRow>>> {
        let mut result: HashMap<i32, Arc<ReservesRow>> = HashMap::new();
        let mut to_fetch: Vec<i32> = Vec::new();
        for id in pool_ids {
            if let Some(cached) = self.ctx.cache.reserves_by_pool_id.get(&id) {
                result.insert(id, Arc::clone(cached));
            } else {
                to_fetch.push(id);
            }
        }
        if !to_fetch.is_empty() {
            let rows = reserves::latest_for_pools(&mut self.ctx.tx, &to_fetch).await?;
            for r in rows {
                let arc = Arc::new(r);
                self.ctx
                    .cache
                    .reserves_by_pool_id
                    .insert(arc.pool_id, Arc::clone(&arc));
                result.insert(arc.pool_id, arc);
            }
        }
        Ok(result)
    }

    pub async fn history(
        &mut self,
        filter: ReservesFilter,
    ) -> sqlx::Result<Vec<Arc<ReservesRow>>> {
        let rows = reserves::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
