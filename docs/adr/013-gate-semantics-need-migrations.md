# ADR-013: Gate-Semantics Changes Ship One-Shot Migrations

**Status:** Accepted
**Date:** 2026-05-06

## Context

`host_rollout_state` rows are written by the dispatch path when a host clears the gates in effect at the time of the dispatch. If gate semantics change later — a predicate like `is_terminal_for_ordering` flips, a new gate is added, an existing gate's verdict shifts — pre-existing rows that were valid at write-time can become invalid against the new semantics. Symptom: a successor channel dispatches even though current `channel_edges` says it shouldn't, because the predecessor row says `Soaked` but the new code wouldn't have promoted that host past `Healthy`.

During v0.2 development (May 4–5 2026), three rounds of gate-semantics fixes shipped in close succession (`5ddaf9f`, `37e8d07`, `74c9ad4`). Each could leave pre-existing rows from the prior gate code as "dirty" against the new code. A *startup invariant pass* (`7ea4a1d`) absorbed this cost: at every CP boot, walk every non-Queued row in `list_in_flight()` rollouts, evaluate gates in `Dispatch` mode, reset to `Queued` on block. The pass logged WARN per reset and was load-bearing on lab during the iteration burn-down (cleaned 10 rows across 3 restarts).

Two classes of dirty rows existed:

- **(a) Recovery partial-write history.** Pre-`7f0078d` orphan-confirm wrote operational + `host_rollout_state` rows in two transactions; failure between them left a confirmed row with no `host_rollout_state` row → snapshot LEFT JOIN projects "Healthy with NULL `last_healthy_since`" → soak timer never fires → host stuck. **Closed structurally by `7f0078d`'s atomic helper. Cannot recur.**
- **(b) Pre-fix race-bug dispatches.** Rows written under earlier gate code (host_edges silent on freshly-opened rollouts; channel_edges asymmetric on missing predecessor). The dispatch already happened; the row is correct-by-construction at the time but invalid against current semantics.

Class (a) is closed forever by code. Class (b) is a workflow problem: every gate-semantics change risks creating a new generation of class-(b) rows, and the invariant pass perpetually absorbs that cost.

## Decision

Delete the startup invariant pass. Future PRs that change gate-semantics MUST ship a one-shot migration in the same PR if the change could create dirty rows.

**"Gate-semantics change"** = any of:
- Predicate change on `HostRolloutState` (`is_terminal_for_ordering`, `is_in_flight`, `is_failed`, `is_active_for_ordering`).
- New gate, removed gate, or changed gate ordering in `nixfleet-reconciler::gates`.
- Change to which rollout-table columns a gate reads (e.g. `terminal_at` vs `superseded_at`).
- Change to the snapshot LEFT JOIN that feeds `Observed` (host_dispatch_state.rs `active_rollouts_snapshot`).

**"Could create dirty rows"** = the new gate code, evaluated against existing rows written pre-fix, could produce a different verdict than the code in production when those rows were written.

**Migration shape:**
- Schema changes: standard versioned file under `crates/nixfleet-control-plane/migrations/` (e.g. `V005__...sql`).
- Semantics-only changes (no schema touch): an idempotent, explicitly-guarded `UPDATE` / `DELETE` shipped either as a versioned migration or as a one-shot routine the PR documents in its description. Re-running must be a no-op.
- The migration is the PR's responsibility. Reviewers flag missing migrations on gate-semantics PRs.

## Consequences

**Positive:**
- 562 LOC + 4 tests removed from the boot path; one fewer thing to reason about during CP startup.
- Gate-semantics changes have a visible, per-PR cost (the migration), aligning with the lean intent for v0.2.
- Dirty rows cannot accumulate silently — they're either prevented by the migration or absent because the change doesn't qualify.

**Negative:**
- The reviewer-discipline check is procedural, not automatic. A reviewer who misses the migration requirement on a gate-semantics PR can re-introduce the class-(b) bug class. Mitigation: this ADR is the canonical reference; link it in PR templates and gate-semantics review comments.
- Iteration on gate predicates during development becomes slightly slower — running fixtures may require re-converging or DB wipes between iterations rather than relying on the pass to clean up between restarts.

**Trade-offs accepted:**
- Class (b) rows that exist on a CP DB at the moment this ADR ships are the operator's responsibility to clean up (one final invariant-pass run before deploying the deletion, or a manual `DELETE` of stale rows). Lab was confirmed clean via SQL inspection on 2026-05-06; prod CPs need their own check.

## Trigger for revisiting

If gate-semantics changes start landing without migrations and produce stuck-host incidents in prod, reconsider re-introducing the pass — possibly as opt-in via env var (`NIXFLEET_RUN_STARTUP_INVARIANT_PASS=1`) so it's available for migration windows without running by default.
