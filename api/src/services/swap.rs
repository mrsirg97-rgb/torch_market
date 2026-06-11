// SwapService — composes PoolService for queries that filter by pool
// metadata (e.g. token mint). Swap rows themselves aren't cached.
// Ported verbatim from deep_pool/indexer.

use std::sync::Arc;

use crate::contracts::SwapRow;
use crate::domain::{swap, SwapFilter};
use crate::services::context::RequestCtx;
use crate::services::pool::PoolService;

pub struct SwapService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> SwapService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn list(&mut self, filter: SwapFilter) -> sqlx::Result<Vec<Arc<SwapRow>>> {
        let rows = swap::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }

    pub async fn for_token_mint(
        &mut self,
        mint: &str,
        limit: Option<i64>,
    ) -> sqlx::Result<Vec<Arc<SwapRow>>> {
        let pools = {
            let mut ps = PoolService::new(&mut *self.ctx);
            ps.for_token_mints(vec![mint.to_string()]).await?
        };
        let pool_ids: Vec<i32> = pools.iter().map(|p| p.pool_id).collect();
        if pool_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = swap::list(
            &mut self.ctx.tx,
            SwapFilter {
                pool_ids: Some(pool_ids),
                limit,
                ..Default::default()
            },
        )
        .await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
