// Domain: swaps (deep_pool). Ported verbatim from deep_pool/indexer.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{NewSwapRow, SwapRow};

#[derive(Default, Debug, Clone)]
pub struct SwapFilter {
    pub ids: Option<Vec<i32>>,
    pub pool_ids: Option<Vec<i32>>,
    pub users: Option<Vec<String>>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

// Idempotent batch insert keyed on (signature, inner_ix_idx). Returns only
// newly inserted rows; replays of already-ingested events return empty.
pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewSwapRow],
) -> sqlx::Result<Vec<SwapRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO swaps (
            pool_id, user_pk, sol_source, is_buy,
            amount_in_gross, amount_in_net, amount_out_gross, amount_out_net,
            fee, sol_reserve_after, token_reserve_after,
            slot, signature, inner_ix_idx, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(row.pool_id)
            .push_bind(&row.user_pk)
            .push_bind(&row.sol_source)
            .push_bind(row.is_buy)
            .push_bind(row.amount_in_gross)
            .push_bind(row.amount_in_net)
            .push_bind(row.amount_out_gross)
            .push_bind(row.amount_out_net)
            .push_bind(row.fee)
            .push_bind(row.sol_reserve_after)
            .push_bind(row.token_reserve_after)
            .push_bind(row.slot)
            .push_bind(&row.signature)
            .push_bind(row.inner_ix_idx)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *");
    qb.build_query_as::<SwapRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM swaps WHERE swap_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
