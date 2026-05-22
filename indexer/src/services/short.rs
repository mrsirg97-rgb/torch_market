// ShortService — current-state lookups for the shorts UI. Symmetric with
// LoanService.

use std::sync::Arc;

use crate::contracts::ShortRow;
use crate::domain::{short, ShortFilter};
use crate::services::context::RequestCtx;

pub struct ShortService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> ShortService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn for_shorter(
        &mut self,
        mint: &str,
        shorter: &str,
    ) -> sqlx::Result<Option<Arc<ShortRow>>> {
        let row = short::get(&mut self.ctx.tx, mint, shorter).await?;
        Ok(row.map(Arc::new))
    }

    pub async fn list(&mut self, filter: ShortFilter) -> sqlx::Result<Vec<Arc<ShortRow>>> {
        let rows = short::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
