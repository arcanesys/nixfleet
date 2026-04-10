# ADR 010 — FleetState Hydration Does Not Load Rollouts

**Date:** 2026-04-10
**Status:** Accepted
**Cycle:** Core hardening (spec: `docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md`, Phase 2)
**Related:** ADR 009 (Category 10)

## Context

During the Phase 1 audit, a suspicious block was found in
`control-plane/src/state.rs::hydrate_from_db`:

```rust
let active_rollouts = db.list_rollouts_by_status(Some("running"), 100)?;
let paused_rollouts = db.list_rollouts_by_status(Some("paused"), 100)?;
```

These lists were loaded from the database and included in a `tracing::info!`
log line, but were **never stored** in `FleetState`. The audit flagged this
as "Investigate" because two interpretations were possible:

1. The lines are a forgotten TODO: someone meant to warm a rollouts cache in
   `FleetState` but only half-wrote it.
2. The lines are diagnostic-only: kept to record counts in the startup log.

## Decision

The lines are diagnostic-only, and are deleted in Phase 2.

**Evidence:** the rollout executor (`control-plane/src/rollout/executor.rs::tick`)
re-queries running rollouts from the database on every 2-second tick:

```rust
let rollouts = db.list_rollouts_by_status(Some("running"), 100)?;
for rollout in rollouts {
    if let Err(error) = process_rollout(state, db, &rollout).await {
        ...
    }
}
```

The executor never reads rollouts from `FleetState`, and `FleetState` never
exposes a rollouts field. A warmed cache would have no reader.

The log line's wording — "Hydrated fleet state from database" with rollout
counts — is also misleading: it implies rollouts are part of FleetState,
which they are not. A future maintainer following the log line would go
looking for an in-memory rollouts field that does not exist.

## Consequences

- Phase 2 deletes the two `list_rollouts_by_status` calls and their
  contribution to the log message.
- The "Hydrated fleet state from database" log now reports only machine
  count, which is the only thing that is actually hydrated.
- F6 (CP restart mid-rollout) scenario in Phase 3 remains valid: the executor
  picks up running rollouts from the DB on its first tick after startup, so
  no explicit hydration is required.
- Should in-memory rollout caching ever become desirable (e.g., to avoid DB
  round-trips per tick), it should be added explicitly to `FleetState` with a
  clear reader path; this ADR documents that the half-baked precursor has
  been removed.
