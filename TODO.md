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
- [x] **`vm-fleet-bootstrap` release generation gate mismatch** — D1 — fixed in this branch. The test created a release whose entries pointed at throwaway `writeTextDir` closures baked into each agent's system, but agents report their real `/run/current-system` toplevel as `current_generation`. The executor's generation gate (`report.generation == release_entry.store_path`) therefore never matched and the rollout paused at batch evaluation. Same bug class as the `vm-fleet.nix` fix above; applied the same pattern (read real toplevels from each agent at test time and build the release body dynamically). The component behaviour is correct — generation gating is a documented core safety property.
- [x] **`vm-fleet-deploy-ssh` store-path sanity assertions are tautological under nixosTest** — D4 — fixed in this branch. `target.fail("test -e <stubPath>")` and the corresponding positive `target.succeed("test -e ...")` were both impossible to fail because nixosTest mounts the host `/nix/store` read-only on every VM via 9p, so any path referenced anywhere in the test evaluation is visible on every node regardless of the node's closure. Dropped the negative check entirely and replaced the positive one with `nix-store -q --references <stubPath>` on the target — that queries the VM-local Nix database, which is NOT shared via 9p, so it actually proves `nix-copy-closure --to` registered the path. Marker file `/tmp/stub-switch-called` (regular filesystem path, VM-local) remains the load-bearing proof that `switch-to-configuration switch` was invoked.
- [x] **`vm-fleet-revert` generation-gate contradiction under dryRun** — F2, C3 — fixed in this branch (pre-emptively). Same bug class as `vm-fleet-bootstrap` and `vm-fleet-apply-failure`: used throwaway `writeTextDir` store paths as release entries. The test's file-level "known caveats" comment claimed the `health_timeout=10` escape hatch would make batch 0 reach `succeeded` via the timeout branch of `evaluate_batch` — that's incorrect; the executor's generation gate (`report.generation == release_entry.store_path`) blocks the batch from succeeding in the first place. Fixed by reading each agent's real `/run/current-system` toplevel at test time and using the distinct per-agent toplevels as the release entries (both exist because nixosTest builds a separate system derivation per node).
- [x] **`vm-fleet-rollback-ssh` store-path sanity assertions are tautological under nixosTest** — RB2 — fixed in this branch (pre-emptively). Same class as `vm-fleet-deploy-ssh`: `target.fail("test -e <stubG1/G2>")` and `target.succeed("test -e <stubG2>")` are invariant because nixosTest mounts the host `/nix/store` read-only via 9p on every VM. Replaced the invariant filesystem checks with `nix-store -q --references <path>` on target, which queries the VM-local Nix database (not shared via 9p) and is therefore load-bearing proof that `nix-copy-closure` registered the path. Marker file `/tmp/stub-switch-last` (VM-local filesystem) remains the proof that each phase's `switch-to-configuration switch` was invoked with the correct generation.
- [x] **`CommandChecker` uses `Command::new("sh")` which fails with ENOENT under systemd** — surfaced by `vm-fleet-apply-failure` — fixed in this branch. **Real product bug.** `agent/src/health/command.rs::CommandChecker::run` invoked `tokio::process::Command::new("sh")` without an absolute path, relying on PATH lookup. When the agent runs as a systemd service on NixOS, the unit's default PATH does not include a directory providing `sh`, so every command health check fails at the fork/exec stage with `failed to run command: No such file or directory (os error 2)` BEFORE the shell even starts parsing the command expression. Every `healthChecks.command` entry is effectively dead on NixOS until an operator customises the service environment — which is not documented anywhere. Fix: use the absolute path `/bin/sh` which is a stable, distro-wide guarantee (on NixOS it is a symlink managed by `environment.binsh`, pointing at bash by default). The systemd / http checks were unaffected because they use distinct code paths (`systemctl is-active` via a different API, and reqwest for HTTP).
- [x] **Rollout executor — stale-report fallback flips batch back to failed on resume** — surfaced by `vm-fleet-apply-failure` F1 Phase 9 — fixed in this branch. **Real product bug.** In `control-plane/src/rollout/executor.rs::evaluate_batch`, the `on_desired_gen` branch's fallback (lines ~242-249) used `recent_reports[0]` without filtering by `received_at >= started_at`. After a paused rollout is resumed via `POST /api/v1/rollouts/{id}/resume`, the deploy path calls `update_batch_status(batch_id, "deploying")` which sets a fresh `started_at`. On the next tick, `evaluate_batch` reads `get_health_reports_since(started_at)` which is empty (agent has not yet sent a fresh report) and falls through to `recent_reports[0]` — but that's the STALE unhealthy report from before resume. The executor then marks the batch failed again and re-pauses the rollout, defeating the resume. Added the symmetric `if r.received_at.as_str() < started_at { pending_count += 1 }` check that the `!on_desired_gen` branch already had. Operator-visible symptom: clearing the underlying problem (e.g., `rm -f /var/lib/fail-next-health`) and resuming produces an instant re-pause before the agent's next poll tick has a chance to send a healthy report.
- [x] **`vm-fleet-apply-failure` generation-gate contradiction under dryRun** — F1, RB1 — fixed in this branch. Same bug class as vm-fleet-bootstrap: the test created a release entry pointing at a throwaway `writeTextDir` store path while the agent runs under `dryRun=true`. Under dryRun the agent literally cannot advance — `apply_generation` is short-circuited at `agent/src/main.rs:302-307` and `current_generation` is read fresh from `/run/current-system` on every report (`main.rs:266`) so it NEVER changes to the release path. The rollout executor's generation gate (`report.generation == release_entry.store_path`) therefore could never match; Phase 5 pause fired for the wrong reason (gate mismatch, not health failure) and Phase 9 resume-to-completed was structurally impossible. Fixed by reading the agent's real `/run/current-system` toplevel at test time (same pattern as vm-fleet-bootstrap / vm-fleet.nix) and using that as the release store path. Rewrote the RB1 assertion to drop the now-tautological `g_mid != RELEASE_PATH` check and instead verify (a) CP still tracks the agent after the paused batch, and (b) `current_generation` did not regress. F1 pause now fires purely from the health-check failure path. Component behaviour is correct and documented.

## Test infrastructure gaps

- [ ] **nixosTest shared-store makes store-path assertions tautological.** Multiple scenario tests would benefit from being able to assert "path X is on node A but not on node B" (e.g. to prove `nix copy` actually transferred something vs. was a no-op). Today that class of assertion is impossible because `/nix/store` is shared read-only via 9p across all VMs. Workarounds used in this branch: (a) VM-local marker files under `/tmp`, (b) Nix-DB queries via `nix-store -q --references` (DB is per-VM, store files are shared). A stronger approach would be to set `virtualisation.useNixStoreImage = true` + `virtualisation.mountHostNixStore = false` per-node to truly isolate stores, but that blows up build time because every VM then rebuilds its own image. Revisit during Phase 4 or later if more tests in this class need to be added.

## Phase 3 — test hardening follow-ups

These are product bugs Phase 3 surfaced via VM-tier scenarios that deserve a
Rust-tier unit or integration test so the regression is caught at
`cargo test --workspace` speed rather than waiting for a ~4 minute VM build.

- [ ] **Rust unit test for `CommandChecker` absolute-path resilience.** The
  bug fixed in this branch (`agent/src/health/command.rs`,
  `Command::new("sh")` → `Command::new("/bin/sh")`) has no direct unit
  test. Add a test that (a) unsets `PATH` in the child environment,
  (b) invokes `CommandChecker::run`, and (c) asserts the result is
  `Pass` (not `Fail { message: "failed to run command: ..." }`). Without
  this, a future contributor could regress the fix by changing to a
  relative path and the existing tests would still pass.

- [ ] **Rust integration test for resume-after-stale-report race.** The
  executor's `evaluate_batch` fix in this branch (`control-plane/src/rollout/executor.rs`,
  `r.received_at.as_str() < started_at` filter on the `on_desired_gen`
  branch) is only covered by the `vm-fleet-apply-failure` VM subtest.
  Add a scenario under `control-plane/tests/failure_scenarios.rs` that:
  (1) seeds a machine with a stale unhealthy report at T0, (2) creates
  a release+rollout that reaches `paused`, (3) POSTs `/resume`, (4)
  ticks the executor ONCE without sending a fresh report, (5) asserts
  the batch did NOT flip to `failed` on the stale report, (6) inserts a
  fresh healthy report, ticks again, (7) asserts the batch reaches
  `succeeded`. Harness already has `tick_once` and `insert_health_report`.

- [ ] **Rust verification pass after executor fix.** The stale-report
  filter changes the `pending_count` vs `unhealthy_count` bookkeeping
  in one branch of `evaluate_batch`. Existing F5 (`f5_failure_threshold_30_percent_pauses_on_4_of_10`)
  uses `get_health_reports_since` which is unaffected, but existing
  F4 (tiebreaker) and F6 (CP restart) indirectly exercise the
  fallback path. Confirm both still pass via `cargo test -p
  nixfleet-control-plane --test failure_scenarios`. If a regression
  appears, it is a test-side stale assumption about the old
  (pre-51ba108) behaviour, not a product regression.

## Post-merge re-verification checklist

Phase 3 ended with a large shared-helpers refactor (`bf53857`) that touched
every VM scenario file and the `vm-fleet.nix` headline test. Before merging
the `hardening/core-scenarios` branch the test suite needs both a
**semantic equivalence audit** (does each refactored test still assert the
same thing the pre-refactor inline version did) and a **runtime
verification** (do the builds actually go green).

### Helper defaults (`modules/tests/_lib/helpers.nix`)

| Helper | Parameter | Default | Notes |
|---|---|---|---|
| `mkCpNode` | `hostName` | `"cp"` | Also used as `certPrefix` unless overridden |
| `mkCpNode` | `certPrefix` | `"cp"` | |
| `mkCpNode` | `extraModules` | `[]` | |
| `mkAgentNode` | `machineId` | `hostName` | |
| `mkAgentNode` | `controlPlaneUrl` | `"https://cp:8080"` | |
| `mkAgentNode` | `tags` | `[]` | |
| `mkAgentNode` | `dryRun` | `true` | |
| `mkAgentNode` | `pollInterval` | `2` | |
| `mkAgentNode` | `healthInterval` | `5` | |
| `mkAgentNode` | `healthChecks` | `{}` | |
| `mkAgentNode` | `agentExtraConfig` | `{}` | `lib.recursiveUpdate`-merged into `services.nixfleet-agent`, wins on collision |
| `mkAgentNode` | `extraAgentModules` | `[]` | Prepended to agent node modules |
| `mkAgentNode` | `extraModules` | `[]` | Appended after agent modules |

### Semantic equivalence audit (already done in this session)

For each refactored file the pre-refactor inline config was diffed against
the post-refactor helper call. Fields listed below are the ones where the
pre-refactor inline config diverged from the helper default and therefore
require an explicit override in the refactored call. All rows are
confirmed **✓ matched** unless flagged.

| File | Divergent field (pre) | Post-refactor handling | Verdict |
|---|---|---|---|
| `tag-sync.nix` | — all defaults | — | ✓ |
| `bootstrap.nix` (web-01/02) | — all defaults | — | ✓ |
| `bootstrap.nix` (operator) | N/A (not an agent) | `mkTestNode + tlsCertsModule { certPrefix="operator"; }` + inline `systemPackages` | ✓ |
| `apply-failure.nix` | `healthInterval = 3` | `healthInterval = 3` explicit, `healthChecks.command` preserved in call | ✓ |
| `revert.nix` (both) | `healthInterval = 3` | Local `mkWebAgent` wrapper passes `healthInterval = 3` + `healthChecks.command` | ✓ |
| `poll-retry.nix` | `pollInterval = 5, retryInterval = 5` | `pollInterval = 5` explicit, `agentExtraConfig.retryInterval = 5` | ✓ |
| `timeout.nix` | `healthInterval = 3`, `wantedBy = lib.mkForce []`, `environment.systemPackages = [web01Closure]` | `healthInterval = 3` explicit, `extraAgentModules = [(_: { systemd.services.nixfleet-agent.wantedBy = lib.mkForce []; environment.systemPackages = [web01Closure]; })]` | ✓ |
| `mtls-missing.nix` (cp) | — all defaults | `mkCpNode` | ✓ |
| `mtls-missing.nix` (unauth) | N/A (client-only node, no agent service) | Kept inline with `mkTestNode + tlsCertsModule { certPrefix="unauth"; }` | ✓ |
| `release.nix` (cp) | — all defaults | `mkCpNode` | ✓ |
| `release.nix` (cache) | N/A (harmonia + sshd scenario-unique) | Kept inline | ✓ |
| `release.nix` (builder) | N/A (client with CLI + nix-shim) | `mkTestNode + tlsCertsModule { certPrefix="builder"; }` + inline for `systemPackages` + `sessionVariables.PATH = lib.mkBefore ["${nixShim}/bin"]` + `environment.etc."ssh-builder-key"` | ✓ |
| `release.nix` (agent) | N/A (`services.nixfleet-cache` consumer, NOT a fleet agent) | `mkTestNode + tlsCertsModule { certPrefix="agent"; }` + inline `services.nixfleet-cache` | ✓ — using `mkAgentNode` here would have been wrong (it enables `services.nixfleet-agent` which this node does not want) |
| `vm-fleet.nix` (web-01/02) | `tags=["web"]`, `healthChecks = webHealthChecks`, `extraAgentModules = webAgentModules` (nginx + node exporter) | Same args passed to shared `mkAgentNode` | ✓ |
| `vm-fleet.nix` (db-01) | `tags=["db"]`, `healthChecks.http = [{url = "http://localhost:9999/health"; ...}]` | Same args | ✓ |

### Runtime verification (NOT yet done)

The equivalence audit above is static; it does not prove the tests still
execute correctly at runtime. Still required:

- [ ] **`nix run .#validate -- --all`** passes end to end. This is the
      one-command shorthand for every check below.
- [ ] **Eval tier** — every `eval-*` check green (unblocked after `0066b2c`).
- [ ] **`vm-fleet-tag-sync`** — agent tags reach the CP via health report.
      Load-bearing: `machine_tags` table rows match the NixOS-declared list.
- [ ] **`vm-fleet-bootstrap`** — first `nixfleet bootstrap` call succeeds
      and seeds an admin API key; a second call returns 409. Load-bearing:
      two-agent rollout reaches `completed` post-bootstrap.
- [ ] **`vm-fleet-release`** — `nixfleet release create --push-to ssh://` runs
      end-to-end; the store path is registered in the cache node's Nix DB
      (not just visible via 9p); the agent substitutes it from harmonia.
- [ ] **`vm-fleet-deploy-ssh`** — `nixfleet deploy --ssh --target` runs
      without any CP in the topology; `nix-store -q --references <stub>`
      succeeds on the target (DB registration proof, not 9p visibility).
- [ ] **`vm-fleet-apply-failure`** — F1 pause via command health check +
      resume → completed via the executor stale-report filter fix
      (`51ba108`) and the `CommandChecker` absolute-`sh` fix (`c49932f`).
      If Phase 9 times out at `paused`, one of those two fixes regressed.
- [ ] **`vm-fleet-revert`** — staged 2-batch rollout with `on_failure=revert`.
      Load-bearing: `previous_generations` on the succeeded batch is a
      non-empty JSON map and the executor walks it to restore the earlier
      generation. Verify the local `mkWebAgent` wrapper produces identical
      nodes for web-01 and web-02.
- [ ] **`vm-fleet-timeout`** — agent unit is NEVER started
      (`wantedBy = []`); batch times out on `pending_count > 0` elapsed
      `health_timeout`. Check that web01Closure is still reachable in
      the system closure via `extraAgentModules` after the refactor.
- [ ] **`vm-fleet-poll-retry`** — agent starts before CP, first poll fails,
      agent retries at `retryInterval = 5` via `agentExtraConfig`, CP comes
      up, agent registers. Check the journal for the retry log line.
- [ ] **`vm-fleet-mtls-missing`** — curl without `--cert` fails at TLS
      handshake; positive control with valid client cert succeeds.
- [ ] **`vm-fleet-rollback-ssh`** — `nixfleet rollback --ssh --generation
      <G1>` runs after a `deploy --ssh` of G2; marker file reflects G1.
- [ ] **`vm-fleet`** — 4-node Tier A fleet test with canary on web tag and
      all-at-once on db tag, pause/resume, metrics. Load-bearing: the
      local `mkAgentNode` that existed pre-refactor is now the shared one
      and `webAgentModules` (nginx + node exporter) are still wired via
      `extraAgentModules`.
- [ ] **`vm-core`** / **`vm-minimal`** / **`vm-infra`** / **`vm-nixfleet`**
      — unchanged by the refactor but not run this session. The
      infrastructure tests inside `vm-infra.nix` include `vm-cache-server`
      and `vm-backup-restic` which were also picked up by dynamic
      discovery for the first time.
- [ ] **`vm-agent-rebuild`** — now uses release + rollout after the
      pre-existing bitrot fix (`eda4315`). Load-bearing: Test B
      "no-cache pre-seeded" reports up-to-date when the release entry
      equals the agent's current toplevel; Test C "missing path guard"
      logs the `fetch_closure` error and does NOT advance the
      `/run/current-system` symlink.
- [ ] **`integration-mock-client`** — simulates a consumer flake importing
      `nixfleet.lib.mkHost`. Unchanged but not run.
- [ ] **`cargo test --workspace`** — all unit + integration tests, with
      particular attention to:
   - `control-plane/tests/failure_scenarios.rs` (F4, F5, F6) after the
     executor stale-report filter change. The filter only fires in the
     `on_desired_gen` branch's fallback, which F4/F5/F6 do not exercise
     directly, but confirm no collateral breakage.
   - Any new Rust-tier guard added for the two bugs (see the "test
     hardening follow-ups" section above — currently deferred).

If any test fails, the semantic audit above narrows the suspect to either
a scenario-specific override that got lost in translation or a
runtime-only concern (executor timing, cache eviction, QEMU flakiness).
Re-check the audit row for the failing scenario first — if the row is
`✓` the fault is runtime, not refactor.

## Phase 4 — checklist coverage

### Rollout semantics inconsistency

- [ ] **`failure_threshold` interpretation is inconsistent between executor code and CLI help.** `control-plane/src/rollout/executor.rs::evaluate_batch` uses `unhealthy_count < threshold { succeed } else { fail }` (line ~311), meaning "threshold N = batch fails if N or more machines are unhealthy". The CLI help text in `cli/src/main.rs:87` says `/// Maximum failures before pausing/reverting`, which implies "allow up to N failures" (i.e., `unhealthy_count <= threshold`). Under the current code, `threshold="0"` is pathological — it can NEVER succeed because `0 < 0` is false regardless of health status. Existing tests use both conventions:
  - `vm-fleet.nix` (web rollout), `revert.nix`, `bootstrap.nix`, `auth_scenarios.rs`: `threshold="1"` with semantic "any single failure fails the batch" (matches current `<` code).
  - `apply-failure.nix` F1 tried `threshold="0"` with semantic "zero tolerance" — broken under current code, workaround was to switch to `"1"`.
  - `failure_scenarios.rs::f5_failure_threshold_30_percent_pauses_on_4_of_10`: comment says "Threshold = ceil(10 * 0.30) = 3. 4 >= 3 → fail" — author's mental model was `>=`, which matches current code.

  Decide one semantic, fix the executor (or the help text) to match, and update the tests consistently. The most operator-intuitive interpretation is probably the `<=` one ("failure_threshold=N means allow up to N failures"), in which case the executor should change `<` to `<=` and any test that depends on the current `<` semantics (revert, vm-fleet web rollout) should decrement its threshold by 1.

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
