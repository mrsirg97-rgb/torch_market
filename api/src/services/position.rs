// PositionService — current-state lookups for the leverage UI (V21). Replaces
// the old LoanService + ShortService: one `positions` table (side enum) plus
// the append-only `position_events` log. Not cached individually; the typical
// query is "all active positions for mint X" which is one DB hit anyway.

use std::sync::Arc;

use crate::contracts::{PositionEventRow, PositionRow, PositionSide};
use crate::domain::{
    position,
    position::event::{self as position_event, PositionEventFilter},
    PositionFilter,
};
use crate::services::context::RequestCtx;

pub struct PositionService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> PositionService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn get(
        &mut self,
        mint: &str,
        owner: &str,
        side: PositionSide,
        position_index: i32,
    ) -> sqlx::Result<Option<Arc<PositionRow>>> {
        let row = position::get(&mut self.ctx.tx, mint, owner, side, position_index).await?;
        Ok(row.map(Arc::new))
    }

    pub async fn list(&mut self, filter: PositionFilter) -> sqlx::Result<Vec<Arc<PositionRow>>> {
        let rows = position::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }

    // Append-only event log — drives /api/liquidations and the per-position
    // history view.
    pub async fn events(
        &mut self,
        filter: PositionEventFilter,
    ) -> sqlx::Result<Vec<Arc<PositionEventRow>>> {
        let rows = position_event::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
