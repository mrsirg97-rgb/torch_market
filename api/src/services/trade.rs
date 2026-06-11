// TradeService — list-only. Trades aren't cached (large, time-ordered).

use std::sync::Arc;

use crate::contracts::TradeRow;
use crate::domain::{trade, TradeFilter};
use crate::services::context::RequestCtx;

pub struct TradeService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> TradeService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn list(&mut self, filter: TradeFilter) -> sqlx::Result<Vec<Arc<TradeRow>>> {
        let rows = trade::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
