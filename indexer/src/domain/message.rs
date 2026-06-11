// Domain: messages (torch). Memo-program events gated on co-presence with a
// torch instruction in the same tx — see stream/decoder.rs.
//
// Append-only, idempotent on (signature, inner_ix_idx).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{MessageRow, NewMessageRow};

#[derive(Default, Debug, Clone)]
pub struct MessageFilter {
    pub ids: Option<Vec<i32>>,
    pub mint: Option<String>,
    pub sender: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn set(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NewMessageRow],
) -> sqlx::Result<Vec<MessageRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO messages (
            mint, sender, memo_text, action_kind,
            slot, signature, inner_ix_idx, created_at
         ) ",
    );
    qb.push_values(rows, |mut b, row| {
        b.push_bind(&row.mint)
            .push_bind(&row.sender)
            .push_bind(&row.memo_text)
            .push_bind(&row.action_kind)
            .push_bind(row.slot)
            .push_bind(&row.signature)
            .push_bind(row.inner_ix_idx)
            .push_bind(row.created_at);
    });
    qb.push(" ON CONFLICT (signature, inner_ix_idx) DO NOTHING RETURNING *");
    qb.build_query_as::<MessageRow>().fetch_all(&mut **tx).await
}

pub async fn del(tx: &mut Transaction<'_, Postgres>, ids: &[i32]) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM messages WHERE message_id = ANY($1)")
        .bind(ids)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
