// MessageService — list-only. Messages aren't cached.

use std::sync::Arc;

use crate::contracts::MessageRow;
use crate::domain::{message, MessageFilter};
use crate::services::context::RequestCtx;

pub struct MessageService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> MessageService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn list(&mut self, filter: MessageFilter) -> sqlx::Result<Vec<Arc<MessageRow>>> {
        let rows = message::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
