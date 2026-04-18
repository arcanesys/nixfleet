# ADR-010: FleetState Hydration Does Not Load Rollouts

**Status:** Accepted
**Date:** 2026-04-10

## Context

`FleetState::hydrate_from_db` loaded active/paused rollouts from the database at startup, logged their counts, but never stored them in `FleetState`. This was either a forgotten TODO (meant to warm a cache) or diagnostic logging.

## Decision

The lines were diagnostic-only and have been removed.

The rollout executor re-queries running rollouts from the database on every 2-second tick. It never reads rollouts from `FleetState`, and `FleetState` never exposes a rollouts field. A warmed cache would have no reader.

## Consequences

- `hydrate_from_db` now hydrates only machines, tags, and desired generations -- the things actually stored in `FleetState`
- The "Hydrated fleet state" log message reports only machine count (the only thing actually hydrated)
- CP restart mid-rollout works correctly: the executor picks up running rollouts from the DB on its first tick
- If in-memory rollout caching ever becomes desirable (e.g., to avoid per-tick DB queries), it should be added explicitly with a clear reader path
