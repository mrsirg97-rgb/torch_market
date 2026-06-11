// MigrationService — one row per migrated mint. Tiny table; not cached.

use std::sync::Arc;

use crate::contracts::MigrationRow;
use crate::domain::{migration, MigrationFilter};
use crate::services::context::RequestCtx;

pub struct MigrationService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> MigrationService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn by_mint(&mut self, mint: &str) -> sqlx::Result<Option<Arc<MigrationRow>>> {
        let row = migration::get_by_mint(&mut self.ctx.tx, mint).await?;
        Ok(row.map(Arc::new))
    }

    pub async fn list(
        &mut self,
        filter: MigrationFilter,
    ) -> sqlx::Result<Vec<Arc<MigrationRow>>> {
        let rows = migration::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
