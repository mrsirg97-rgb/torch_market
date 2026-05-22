// LiquidityService — same shape as SwapService, for liquidity events.
// Ported verbatim from deep_pool/indexer.

use std::sync::Arc;

use crate::contracts::LiquidityRow;
use crate::domain::{liquidity, LiquidityFilter};
use crate::services::context::RequestCtx;
use crate::services::pool::PoolService;

pub struct LiquidityService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> LiquidityService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn list(
        &mut self,
        filter: LiquidityFilter,
    ) -> sqlx::Result<Vec<Arc<LiquidityRow>>> {
        let rows = liquidity::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }

    pub async fn for_token_mint(
        &mut self,
        mint: &str,
        limit: Option<i64>,
    ) -> sqlx::Result<Vec<Arc<LiquidityRow>>> {
        let pools = {
            let mut ps = PoolService::new(&mut *self.ctx);
            ps.for_token_mints(vec![mint.to_string()]).await?
        };
        let pool_ids: Vec<i32> = pools.iter().map(|p| p.pool_id).collect();
        if pool_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = liquidity::list(
            &mut self.ctx.tx,
            LiquidityFilter {
                pool_ids: Some(pool_ids),
                limit,
                ..Default::default()
            },
        )
        .await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
