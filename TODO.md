# TODO

Future work discovered during implementation. Grouped by target phase.

## Phase 3 — scenario tests

**Status:** awaiting review (branch `hardening/core-scenarios`, plan `docs/superpowers/plans/2026-04-10-core-hardening-phase-3-scenarios.md`). All 11 Rust-tier scenario files + 10 VM-tier subtests landed; Rust tests run green; VM eval is clean; runtime VM builds still pending user-side verification.

### Phase 3 bugs surfaced

_Track every bug a scenario exposes here. Format: `- [<status>] <one-liner> — <scenario id> — <commit or issue link>`._

Out of Phase 2's original contingent scenarios (C1–C3):
- **C1 (policy create + rollout)** — dropped; policy subsystem was deleted in Phase 2
- **C2 (schedule creation + executor pickup)** — dropped; schedule subsystem was deleted in Phase 2
- **C3 (agent health check subsystem)** — kept; folded into Task 22b (`vm-fleet-revert`) which requires post-apply `run_all` to trigger the revert path

- [x] **`get_recent_reports` non-deterministic tiebreaker** — F4 — fixed in this branch. `ORDER BY received_at DESC` was not enough when two reports arrived in the same wall-clock second (TEXT column with `datetime('now')` second precision). Added `id DESC` secondary sort so "latest wins" is deterministic under sub-second collisions.
- [x] **`get_machines_by_tags` did not filter by lifecycle** — M1 — fixed in this branch. The query joined only `machine_tags`, so a decommissioned machine still tagged `web` was returned as a rollout target. Added an `INNER JOIN machines` with `m.lifecycle = 'active'` so only active machines are targetable. ADR 009 Category 1 `Test` verdict cleared.
- [x] **`vm-fleet.nix` stale request shape** — existing Phase 2 VM test — fixed in this branch. Since PR #28 (release abstraction), `CreateRolloutRequest` takes `release_id` instead of `generation_hash`; `vm-fleet.nix` still posted the pre-#28 shape. The test eval-checked clean but 400'd at runtime. Updated it to create a real release first (whose entries reference each agent's actual `/run/current-system` toplevel, so the generation gate matches under `dryRun=true`), then post the rollout with `release_id`.
- [x] **`register_machine` lifecycle default footgun** — vm-fleet.nix — fixed in this branch. `RegisterMachineRequest.lifecycle` defaulted to `"pending"` when the field was omitted. The CLI's `nixfleet machines register` sends `{"tags": [...]}` with no lifecycle field, implicitly expecting the machine to be immediately targetable. Combined with the M1 lifecycle filter above, an operator would register a machine and then hit a 400 "no machines match the target" on their next deploy with no obvious cause. Changed the default to `"active"`. Operators who need the reservation path pass `lifecycle: "pending"` explicitly.

## Phase 4 — checklist coverage

### CLI gaps surfaced during Phase 2 verification

- [ ] **Env-var precedence in CLI `config::resolve`** — `NIXFLEET_CONTROL_PLANE_URL`, `NIXFLEET_API_KEY`, `NIXFLEET_CA_CERT`, `NIXFLEET_CLIENT_CERT`, `NIXFLEET_CLIENT_KEY` are documented in CLAUDE.md but not enforced in `resolve`. Phase 3 I2 left `i2_env_var_precedence_deferred` as `#[ignore]` pending a Phase 4 fix. Wire env-var layer between credentials and CLI args.
- [ ] **`nixfleet release delete` subcommand** — `DELETE /api/v1/releases/{id}` exists on the CP (documented in CLAUDE.md, role: admin, returns 409 if referenced by a rollout), but there is no matching CLI subcommand. Phase 2 end-to-end smoke revealed `nixfleet release delete --help` returns `unrecognized subcommand 'delete'`. Add the subcommand to `cli/src/release.rs` and wire it in `cli/src/main.rs`. Test case: Phase 3 scenario `R5` (delete on referenced release → 409) + `R6` (delete on orphan → 204).

### Agent UX

- [ ] **Agent's `Failed to check desired generation: control plane returned error status` warning on a fresh DB.** Endpoint returns a 404-style "no generation set" response, agent treats as a hard error and schedules a 30s retry. The agent recovers automatically as soon as any desired generation is set. The warning is cosmetic but noisy after every CP DB wipe or first-boot. Fix: distinguish "endpoint returned 4xx no-state" from "real error" in the agent's poll loop and log at INFO or DEBUG instead of WARN for the first-boot case. See Phase 2 verification (2026-04-10) audit log for reproduction: agent spammed this warn from 17:00:11 until 17:04:38 when the first generation was set.

### Defense-in-depth

- [ ] **CN validation on mTLS.** Currently any cert signed by the fleet CA is accepted for any agent identity; the agent identifies itself via URL path (`/api/v1/machines/{id}/report`). Add a check that the cert CN matches the `{id}` in the path for agent-facing mTLS routes.

### Module ergonomics

- [ ] **`services.nixfleet-cache-server.signingKeyFile` leaks the harmonia user detail.** The option takes a path but does not document that the upstream `services.harmonia.cache` module runs as the `harmonia` system user, so the file must be readable by that user (not root-only). Every operator writing `signingKeyFile = "/run/secrets/cache-key"` will hit "harmonia cannot read file" on first start until they discover the permission requirement. Options: (a) document the permission requirement in the option description; (b) add a `generateIfMissing = true` option that creates the key under `/var/lib/harmonia/` with the correct ownership on first boot; (c) wrap the upstream module to automatically `chown harmonia` the configured path via a preStart hook. Surfaced by Phase 3 `vm-fleet-release` VM test.

## Infrastructure / dependencies

- [ ] **#22: Revert `attic` input to upstream** when https://github.com/zhaofengli/attic/pull/300 is merged. Currently pinned to a fork.
- [ ] **Cosmetic:** Generation count fix in compliance probes (count the active system as generation 1, not 0).

## Testability refactor gap (deferred per spec Section 6 R1)

Spec `docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md` Section 6 R1 forbids testability refactors inside the hardening cycle. Phase 3 surfaced one gap that is blocked by this rule:

- [ ] **Agent poll-loop lib extraction for in-process P1/P2.** The `poll_hint` honouring logic lives in `agent/src/main.rs` (around the `PollOutcome::Success { poll_hint: Some(hint) }` match arm, ~lines 170–186 at PR #30). Testing whether the real agent honours `poll_hint` requires observing its poll cadence, which in turn requires either:
  - Fake time (`tokio::time::pause()`), which does not work across OS processes, OR
  - Extracting the loop into `pub async fn agent::run_loop(...)` in a new `agent/src/lib.rs` so an in-process `#[tokio::test]` can drive it deterministically.

  The refactor is the right solution; it is deferred to a follow-up testability cycle. CP-side emission of `poll_hint` IS covered in Phase 3 by `control-plane/tests/polling_scenarios.rs` (scenarios P1, P2) so the CP half of the contract is under test; only the agent half is gapped.
