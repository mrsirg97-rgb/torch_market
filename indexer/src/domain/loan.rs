// Domain: loans (torch). Current-state table — one row per (mint, borrower),
// UPSERTed on every loan event. Closed positions remain as rows with
// is_active=false (audit trail). Never DELETE.

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{LoanRow, NewLoanRow, PositionHealth};

#[derive(Default, Debug, Clone)]
pub struct LoanFilter {
    pub mint: Option<String>,
    pub borrower: Option<String>,
    pub health: Option<PositionHealth>,
    pub is_active: Option<bool>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    borrower: &str,
) -> sqlx::Result<Option<LoanRow>> {
    sqlx::query_as::<_, LoanRow>("SELECT * FROM loans WHERE mint = $1 AND borrower = $2")
        .bind(mint)
        .bind(borrower)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: LoanFilter,
) -> sqlx::Result<Vec<LoanRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM loans WHERE 1=1");
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(borrower) = filter.borrower {
        qb.push(" AND borrower = ").push_bind(borrower);
    }
    if let Some(health) = filter.health {
        qb.push(" AND health = ").push_bind(health);
    }
    if let Some(is_active) = filter.is_active {
        qb.push(" AND is_active = ").push_bind(is_active);
    }
    qb.push(" ORDER BY updated_at DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<LoanRow>().fetch_all(&mut **tx).await
}

// UPSERT on (mint, borrower). On conflict, every mutable field updates; the
// PK + created_at are preserved. Returns the resulting row.
pub async fn upsert(
    tx: &mut Transaction<'_, Postgres>,
    row: &NewLoanRow,
) -> sqlx::Result<LoanRow> {
    sqlx::query_as::<_, LoanRow>(
        "INSERT INTO loans (
            mint, borrower, collateral_amount, borrowed_amount,
            accrued_interest_stored, last_update_slot,
            health, is_active, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (mint, borrower) DO UPDATE SET
             collateral_amount = EXCLUDED.collateral_amount,
             borrowed_amount = EXCLUDED.borrowed_amount,
             accrued_interest_stored = EXCLUDED.accrued_interest_stored,
             last_update_slot = EXCLUDED.last_update_slot,
             health = EXCLUDED.health,
             is_active = EXCLUDED.is_active,
             updated_at = EXCLUDED.updated_at
         RETURNING *",
    )
    .bind(&row.mint)
    .bind(&row.borrower)
    .bind(row.collateral_amount)
    .bind(row.borrowed_amount)
    .bind(row.accrued_interest_stored)
    .bind(row.last_update_slot)
    .bind(row.health)
    .bind(row.is_active)
    .bind(row.created_at)
    .bind(row.updated_at)
    .fetch_one(&mut **tx)
    .await
}
