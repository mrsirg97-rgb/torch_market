// Domain: positions (torch, V21). Unified current-state table for leverage
// positions — one on-chain `Position` struct (side = long | short), keyed by
// (mint, owner, side, position_index). UPSERTed on every open/close/liquidate.
// Closed/liquidated positions remain as rows with is_active=false (audit
// trail); never DELETE. `owner` may be a wallet or a TorchVault PDA
// (owner_is_vault). Companion append-only log lives in the `event` submodule.

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::contracts::{
    PositionEventKind, PositionHealth, PositionRow, PositionSide,
};

#[derive(Default, Debug, Clone)]
pub struct PositionFilter {
    pub mint: Option<String>,
    pub owner: Option<String>,
    pub side: Option<PositionSide>,
    pub health: Option<PositionHealth>,
    pub is_active: Option<bool>,
    pub limit: Option<i64>,
}

pub async fn get(
    tx: &mut Transaction<'_, Postgres>,
    mint: &str,
    owner: &str,
    side: PositionSide,
    position_index: i32,
) -> sqlx::Result<Option<PositionRow>> {
    sqlx::query_as::<_, PositionRow>(
        "SELECT * FROM positions
         WHERE mint = $1 AND owner = $2 AND side = $3 AND position_index = $4",
    )
    .bind(mint)
    .bind(owner)
    .bind(side)
    .bind(position_index)
    .fetch_optional(&mut **tx)
    .await
}

pub async fn list(
    tx: &mut Transaction<'_, Postgres>,
    filter: PositionFilter,
) -> sqlx::Result<Vec<PositionRow>> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM positions WHERE 1=1");
    if let Some(mint) = filter.mint {
        qb.push(" AND mint = ").push_bind(mint);
    }
    if let Some(owner) = filter.owner {
        qb.push(" AND owner = ").push_bind(owner);
    }
    if let Some(side) = filter.side {
        qb.push(" AND side = ").push_bind(side);
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
    qb.build_query_as::<PositionRow>().fetch_all(&mut **tx).await
}

// Append-only leverage event log (`position_events`) — analog of `trades`.
// Preserves per-event analytics (bad_debt, twap_ltv, bonus_bps, seized) that
// the current-state `positions` table can't retain. Idempotent on
// (signature, inner_ix_idx) so backfill re-runs are safe.
pub mod event {
    use super::*;
    use crate::contracts::PositionEventRow;

    #[derive(Default, Debug, Clone)]
    pub struct PositionEventFilter {
        pub mint: Option<String>,
        pub owner: Option<String>,
        pub side: Option<PositionSide>,
        pub kind: Option<PositionEventKind>,
        pub limit: Option<i64>,
    }

    pub async fn list(
        tx: &mut Transaction<'_, Postgres>,
        filter: PositionEventFilter,
    ) -> sqlx::Result<Vec<PositionEventRow>> {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT * FROM position_events WHERE 1=1");
        if let Some(mint) = filter.mint {
            qb.push(" AND mint = ").push_bind(mint);
        }
        if let Some(owner) = filter.owner {
            qb.push(" AND owner = ").push_bind(owner);
        }
        if let Some(side) = filter.side {
            qb.push(" AND side = ").push_bind(side);
        }
        if let Some(kind) = filter.kind {
            qb.push(" AND kind = ").push_bind(kind);
        }
        qb.push(" ORDER BY slot DESC, event_id DESC");
        if let Some(limit) = filter.limit {
            qb.push(" LIMIT ").push_bind(limit);
        }
        qb.build_query_as::<PositionEventRow>()
            .fetch_all(&mut **tx)
            .await
    }

}
