# TODO

Future work discovered during implementation. Grouped by target phase.

## Phase 3 — scenario tests

**Status:** in progress (branch `hardening/core-scenarios`, plan `docs/superpowers/plans/2026-04-10-core-hardening-phase-3-scenarios.md`).

### Phase 3 bugs surfaced

_Track every bug a scenario exposes here. Format: `- [<status>] <one-liner> — <scenario id> — <commit or issue link>`._

Out of Phase 2's original contingent scenarios (C1–C3):
- **C1 (policy create + rollout)** — dropped; policy subsystem was deleted in Phase 2
- **C2 (schedule creation + executor pickup)** — dropped; schedule subsystem was deleted in Phase 2
- **C3 (agent health check subsystem)** — kept; folded into Task 22b (`vm-fleet-revert`) which requires post-apply `run_all` to trigger the revert path

- [x] **`get_recent_reports` non-deterministic tiebreaker** — F4 — fixed in this branch. `ORDER BY received_at DESC` was not enough when two reports arrived in the same wall-clock second (TEXT column with `datetime('now')` second precision). Added `id DESC` secondary sort so "latest wins" is deterministic under sub-second collisions.
- [x] **`get_machines_by_tags` did not filter by lifecycle** — M1 — fixed in this branch. The query joined only `machine_tags`, so a decommissioned machine still tagged `web` was returned as a rollout target. Added an `INNER JOIN machines` with `m.lifecycle = 'active'` so only active machines are targetable. ADR 009 Category 1 `Test` verdict cleared.

## Phase 4 — checklist coverage

### CLI gaps surfaced during Phase 2 verification

- [ ] **`nixfleet release delete` subcommand** — `DELETE /api/v1/releases/{id}` exists on the CP (documented in CLAUDE.md, role: admin, returns 409 if referenced by a rollout), but there is no matching CLI subcommand. Phase 2 end-to-end smoke revealed `nixfleet release delete --help` returns `unrecognized subcommand 'delete'`. Add the subcommand to `cli/src/release.rs` and wire it in `cli/src/main.rs`. Test case: Phase 3 scenario `R5` (delete on referenced release → 409) + `R6` (delete on orphan → 204).

### Agent UX

- [ ] **Agent's `Failed to check desired generation: control plane returned error status` warning on a fresh DB.** Endpoint returns a 404-style "no generation set" response, agent treats as a hard error and schedules a 30s retry. The agent recovers automatically as soon as any desired generation is set. The warning is cosmetic but noisy after every CP DB wipe or first-boot. Fix: distinguish "endpoint returned 4xx no-state" from "real error" in the agent's poll loop and log at INFO or DEBUG instead of WARN for the first-boot case. See Phase 2 verification (2026-04-10) audit log for reproduction: agent spammed this warn from 17:00:11 until 17:04:38 when the first generation was set.

### Defense-in-depth

- [ ] **CN validation on mTLS.** Currently any cert signed by the fleet CA is accepted for any agent identity; the agent identifies itself via URL path (`/api/v1/machines/{id}/report`). Add a check that the cert CN matches the `{id}` in the path for agent-facing mTLS routes.

## Infrastructure / dependencies

- [ ] **#22: Revert `attic` input to upstream** when https://github.com/zhaofengli/attic/pull/300 is merged. Currently pinned to a fork.
- [ ] **Cosmetic:** Generation count fix in compliance probes (count the active system as generation 1, not 0).
