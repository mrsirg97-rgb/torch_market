// Domain: messages (torch). Memo-program events gated on co-presence with a
// torch instruction in the same tx — see stream/decoder.rs.
//
// Append-only, idempotent on (signature, inner_ix_idx).

use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::MessageRow;

#[derive(Default, Debug, Clone)]
pub struct MessageFilter {
    pub ids: Option<Vec<i32>>,
    pub mint: Option<String>,
    pub sender: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    message_id: i32,
) -> sqlx::Result<Option<MessageRow>> {
    sqlx::query_as::<_, MessageRow>("SELECT * FROM messages WHERE message_id = $1")
        .bind(message_id)
        .fetch_optional(&mut **tx)
        .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: MessageFilter,
) -> sqlx::Result<Vec<MessageRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM messages WHERE 1=1");
    if let Some(ids) = filter.ids {
        qb.push(" AND message_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(sender) = filter.sender {
        qb.push(" AND sender = ").push_bind(sender);
    }
    if let Some(since) = filter.since {
        qb.push(" AND created_at > ").push_bind(since);
    }
    if let Some(before) = filter.before {
        qb.push(" AND created_at < ").push_bind(before);
    }
    qb.push(" ORDER BY slot DESC, message_id DESC");
    if let Some(limit) = filter.limit {
        qb.push(" LIMIT ").push_bind(limit);
    }
    qb.build_query_as::<MessageRow>().fetch_all(&mut **tx).await
}
