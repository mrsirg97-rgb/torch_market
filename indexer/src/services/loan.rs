// LoanService — current-state lookups for the lending UI. Not cached
// individually; the typical query is "all active loans for mint X" which
// is one DB hit anyway.

use std::sync::Arc;

use crate::contracts::LoanRow;
use crate::domain::{loan, LoanFilter};
use crate::services::context::RequestCtx;

pub struct LoanService<'a> {
    ctx: &'a mut RequestCtx,
}

impl<'a> LoanService<'a> {
    pub fn new(ctx: &'a mut RequestCtx) -> Self {
        Self { ctx }
    }

    pub async fn for_borrower(
        &mut self,
        mint: &str,
        borrower: &str,
    ) -> sqlx::Result<Option<Arc<LoanRow>>> {
        let row = loan::get(&mut self.ctx.tx, mint, borrower).await?;
        Ok(row.map(Arc::new))
    }

    pub async fn list(&mut self, filter: LoanFilter) -> sqlx::Result<Vec<Arc<LoanRow>>> {
        let rows = loan::list(&mut self.ctx.tx, filter).await?;
        Ok(rows.into_iter().map(Arc::new).collect())
    }
}
