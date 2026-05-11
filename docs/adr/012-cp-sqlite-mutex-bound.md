# ADR-012: Mutex<Connection> for the Control-Plane SQLite, Bounded to ~150 Hosts

**Status:** Accepted
**Date:** 2026-05-01

## Context

The control plane keeps operational state (host_reports, host_rollout_state, pending_confirms, token_replay, certificate_revocations, ...) in a single SQLite database. The DB handle is wrapped in `tokio::sync::Mutex<rusqlite::Connection>` (`crates/nixfleet-control-plane/src/db.rs:84`). WAL mode is enabled, so reads can proceed while a write is in flight at the file level - but every operation that goes through the `Mutex<Connection>` serializes on the mutex itself.

A self-bounding line in `db.rs` declares "A single `Mutex<Connection>` is sufficient for fleet sizes O(100)." That bound was set by inspection during v0.2's reconciler and dispatch design; it has not been load-tested.

The cap is architecturally invisible. A future operator scaling past ~200 hosts has no signal that dispatch bursts will queue on the mutex; the symptom is "the system gets slow," not "broken." This ADR records the bound, the rationale, the migration trigger, and the migration target.

## Decision

For v0.2 we keep `Mutex<rusqlite::Connection>` and document the bound:

- **Target operating envelope.** ≤ 150 hosts checking in at the configured polling cadence (default 60s with jitter). Past that, dispatch bursts and report ingestion start contending on the mutex; queueing is acceptable as long as p99 dispatch latency stays under one polling cycle. The bound is conservative - the cycle headroom in normal operation is large - but it is the documented commitment.
- **Why this shape, this cycle.** Single deploy, single operator, no pool dependency to vet. SQLite + WAL + a single connection has predictable concurrency semantics (no transaction-interleaving questions, no connection-affinity bugs). Adding a pool at this scale would buy negligible throughput and cost a moderate dependency review (`r2d2-sqlite` or `deadpool-sqlite`).
- **Migration trigger.** Move to a connection pool when **any** of the following holds:
  1. Fleet size > 150 hosts in production.
  2. p99 of `dispatch_for_host` (once instrumented via `tracing::span!`) exceeds one polling cycle (default 60s) in steady state.
  3. Operator sees rollout-burst contention in the journal - characterised as a tick whose `actions = N` produces visible queueing at the agent log level (agents reporting check-in latencies > 5s without a network or TLS root cause).
- **Migration target.** `deadpool-sqlite` over `r2d2-sqlite`. Both expose the same `rusqlite::Connection` surface, so the migration is a swap of the wrapper plus an `await` per use site. `deadpool-sqlite` integrates natively with tokio (`async fn get()` instead of blocking `get()`); the CP is a tokio app, so this matches the rest of the codebase. `r2d2-sqlite` uses synchronous waits and would force a `spawn_blocking` wrapper at every call site - net friction, not gain.
- **Visibility today.** At startup, when the verified-fleet snapshot is primed from the channel-refs source, the CP logs the host count. Operators see the curve over time in the journal without parsing the DB. This is the cheapest signal that gets us to "we know when the bound matters."

## Consequences

**Positive:**
- No new dependency for v0.2; the workspace stays minimal.
- The mutex is the single source of truth for write ordering - debuggable with `tracing` instrumentation when needed.
- Migration is a non-breaking refactor when it lands: same SQL, same schema, same behaviour, just multi-connection on the inside.

**Negative:**
- The bound is invisible to operators without log-line discipline. The host-count log on startup is the minimum visibility - finer-grained signals (queue depth, lock-wait time) are not instrumented today and would have to land alongside the migration.
- A misconfigured fleet that grows past the bound silently does not break - it gets slow. The migration trigger relies on operator observation rather than an automatic fail-loud.

**Trade-offs accepted:**
- Observability today is bounded to the host-count log; finer instrumentation lands with the pool migration when contention is the dominant cost.
- The 150-host figure is conservative against the current fleet (lab + krach + ohm + pixel + aether = 5 hosts). The wide margin gives the v0.3 pool migration room to take its time.

## Migration plan (v0.3)

When the trigger fires, the migration is:

1. Add `deadpool-sqlite` to the workspace; pin to a release on the maintained branch.
2. Replace `Mutex<Connection>` in `crates/nixfleet-control-plane/src/db.rs` with a pool typed `deadpool_sqlite::Pool`.
3. Replace `let conn = self.conn.lock().await` with `let conn = self.pool.get().await?` at every call site (mechanical).
4. Add `tracing::span!` spans on the hot dispatch path; ship a load-test scenario in the microvm harness asserting p99 dispatch latency under N synthetic hosts.
5. Append a "v0.3 update" section to this ADR documenting the new bound (pool size becomes the new bound).
6. No behavioural changes to public API.
