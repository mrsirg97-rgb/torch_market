// MarketService — read-side composition for the markets table. Caches
// by mint (markets are the primary "find this thing" lookup for every
// per-mint API endpoint).

use std::sync::Arc;

use crate::contracts::MarketRow;
use crate::domain::{market, MarketFilter};
use crate::services::context::RequestCtx;

pub struct MarketService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> MarketService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn by_mint(&mut self, mint: &str) -> sqlx::Result<Option<Arc<MarketRow>>> {
        if let Some(cached) = self.ctx.cache.markets_by_mint.get(mint) {
            return Ok(Some(Arc::clone(cached)));
        }
        let Some(row) = market::get_by_mint(&mut self.ctx.tx, mint).await? else {
            return Ok(None);
        };
        Ok(Some(self.cache_one(row)))
    }

    pub async fn list(&mut self, filter: MarketFilter) -> sqlx::Result<Vec<Arc<MarketRow>>> {
        let rows = market::list(&mut self.ctx.tx, filter).await?;
        Ok(self.cache_many(rows))
    }

    fn cache_one(&mut self, row: MarketRow) -> Arc<MarketRow> {
        let arc = Arc::new(row);
        self.ctx
            .cache
            .markets_by_mint
            .insert(arc.mint.clone(), Arc::clone(&arc));
        arc
    }

    fn cache_many(&mut self, rows: Vec<MarketRow>) -> Vec<Arc<MarketRow>> {
        rows.into_iter().map(|r| self.cache_one(r)).collect()
    }
}
