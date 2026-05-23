# Indexer HA Design (Future)

Not implemented. Premature for current scale. Document captures the design
so the future migration is mechanical rather than re-litigated.

## Current state (single-process)

```
┌─────────────────────────────────────────────────────────────┐
│  one process                                                │
│   ┌──────────┐   ┌────────┐   ┌──────────┐   ┌───────────┐  │
│   │ gRPC sub │──▶│ writer │──▶│ Postgres │   │ axum API  │  │
│   └──────────┘   └───┬────┘   └──────────┘   │ + WS /events│ │
│                      │                       └─────▲─────┘  │
│                      │ tokio::broadcast            │        │
│                      └─────────────────────────────┘        │
└─────────────────────────────────────────────────────────────┘
```

Correct under leader-of-one semantics. Fine for pre-launch and early
production. Single point of failure: process crash → ingestion stops.

## Target HA state

```
                          ┌──────────────────┐
                          │  Postgres        │
                          │  (state + lock)  │
                          └──┬──────────┬────┘
        pg_advisory_lock ───▶│          │◀─── LISTEN torch_events
                             │          │
   ┌─────────────────────────┴┐  ┌──────┴───────────────────┐
   │  indexer instance #1     │  │  indexer instance #N     │
   │  (leader — has lock)     │  │  (read-only)             │
   │   ┌──────┐  ┌──────┐     │  │   ┌──────────────────┐   │
   │   │ gRPC │─▶│writer│─NOTIFY  │   │ LISTEN listener  │   │
   │   └──────┘  └──┬───┘     │  │   └────┬─────────────┘   │
   │                │ tokio::broadcast    │ tokio::broadcast│
   │              ┌─▼──────┐  │  │      ┌─▼──────┐          │
   │              │ axum + │  │  │      │ axum + │          │
   │              │  WS    │  │  │      │  WS    │          │
   │              └────────┘  │  │      └────────┘          │
   └──────────────────────────┘  └──────────────────────────┘
```

- **One writer at a time.** Leader holds a Postgres session-bound
  advisory lock; on TCP disconnect Postgres auto-releases, next poller
  grabs it.
- **All instances serve reads + WS.** No sticky load balancing required.
- **Broadcast crosses processes via Postgres `LISTEN`/`NOTIFY`.** Writer
  issues `NOTIFY torch_events, '<json>'` post-commit; all instances
  `LISTEN`; received notifications are re-broadcast on each instance's
  local `tokio::broadcast` to its WS clients.

## Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | Single writer via `pg_try_advisory_lock` | Already on Postgres; lock is session-bound (no TTL, no split-brain races); zero new infrastructure. |
| 2 | Active-passive, not active-active | Idempotent `ON CONFLICT (signature, inner_ix_idx) DO NOTHING` makes active-active correct, but wastes 2-N× gRPC bandwidth + decode CPU per instance. Single writer is cheaper. |
| 3 | Split-brain during election windows is acceptable | If two instances briefly both believe they're leader (race), idempotent writes mean correctness is preserved. Election can be lazy (5s poll) without affecting correctness — only cost. |
| 4 | Cross-process broadcast via Postgres `LISTEN`/`NOTIFY`, not Redis | One less infrastructure component. Postgres holds NOTIFY until commit (post-commit ordering is automatic). Payload limit 8KB / single channel per DB — comfortable for our event rates. Migrate to Redis only if metrics force it. |
| 5 | Two-layer fan-out: LISTEN cross-instance, `tokio::broadcast` in-instance | WS handler code unchanged. Single LISTEN connection per instance → local broadcast → per-client WS. |
| 6 | Each instance subscribes to gRPC speculatively; only the leader writes | Leader-of-one means non-leader's writes would be no-ops anyway. But the gRPC subscription cost is not worth duplicating — non-leaders should NOT subscribe to Yellowstone. Saves N× gRPC bandwidth. |
| 7 | Read API + WS run on every instance | Stateless reads against shared Postgres. Horizontal scaling for read traffic is free once the HA shell is in. |

## Implementation outline

Three additions to the existing crate:

**1. Leader election (`stream/leader.rs`, ~40 lines)**

```rust
// Acquire and hold pg_advisory_lock on a fixed key. On loss, returns to
// caller (which restarts the loop). The lock is session-bound — when the
// dedicated connection dies, Postgres releases automatically.
pub async fn run_as_leader<F, Fut>(
    db: &PgPool,
    lock_key: i64,
    poll_interval: Duration,
    run: F,
) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
```

`main.rs` wraps the gRPC + writer tasks in `run_as_leader`. Non-leader
instances skip gRPC/writer entirely and only run the API + LISTEN
listener.

**2. Writer NOTIFY (modify `stream/writer.rs`, ~5 lines)**

Replace each `broadcaster.publish(frame)` with a Postgres NOTIFY after
commit:

```rust
sqlx::query("SELECT pg_notify('torch_events', $1)")
    .bind(serde_json::to_string(&frame)?)
    .execute(&db).await?;
```

The local `tokio::broadcast` stays for in-process subscribers (WS clients
on the writer instance). The NOTIFY adds cross-process reach.

**3. LISTEN forwarder (`api/listen.rs`, ~30 lines)**

Each instance (leader or not) runs a single LISTEN task:

```rust
async fn run_listen(db_url: &str, broadcaster: Broadcaster) {
    let mut listener = PgListener::connect(db_url).await?;
    listener.listen("torch_events").await?;
    while let Ok(notification) = listener.recv().await {
        if let Ok(frame) = serde_json::from_str::<BroadcastFrame>(
            notification.payload(),
        ) {
            broadcaster.publish(frame);
        }
    }
}
```

The WS handler code (subscribes to `Broadcaster`) is unchanged.

## Operational notes

- **Postgres connection pool through pgbouncer**: `LISTEN`/`NOTIFY`
  requires session-bound connections. pgbouncer in transaction-pool mode
  silently drops notifications. Either run the listener through a
  dedicated session-pool, connect directly to Postgres, or use a
  long-lived sqlx connection (`sqlx::PgConnection` not pooled). The
  writer's NOTIFY can go through the regular pool — it's a one-shot
  query.
- **Advisory lock key**: pick a static `i64` (e.g.,
  `0xT0R3H1ND3X3R` packed) and document it. One key = one writer
  per Postgres database. If you ever run two separate indexer flavors
  in the same DB (different program sets), give them distinct keys.
- **Leader handoff window**: on graceful shutdown, the leader can
  explicitly release the lock (`pg_advisory_unlock`) for instant
  handoff. On crash, the session disconnect triggers auto-release after
  TCP keepalive timeout (Postgres default ~5min, tunable with
  `tcp_keepalives_idle`). For faster crash recovery, set the TCP
  keepalive aggressively on the writer's lock-holder connection.

## When to revisit

Don't pre-build this. Tripwires:
- Indexer crashed and lost > 5 min of events before being restarted.
- Mainnet traffic forces >1 WS server for throughput.
- Need read-after-write consistency across instances (already satisfied
  by shared Postgres, but worth confirming with real load).

## Future: Redis pub/sub as a follow-up

If `LISTEN`/`NOTIFY` saturates (1000+ subscribers, very high event
rates), migrate to Redis pub/sub. The abstraction "publish on commit,
subscribe per API instance" is identical — the migration touches only
the writer's publish line and the API listener task. The WS handler
remains untouched. Estimate: half a day of work, deferred until metrics
prove the need.
