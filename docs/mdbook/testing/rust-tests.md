# Rust Tests

The Rust side of nixfleet lives in three crates:

| Crate | Path | Role |
|---|---|---|
| `nixfleet-control-plane` | `control-plane/` | Axum HTTP server, SQLite state, rollout executor, release registry, auth/audit, metrics |
| `nixfleet-agent` | `agent/` | Polling daemon, health check runners, store/TLS |
| `nixfleet-types` | `shared/` | Wire types shared by the CLI, agent, and CP |

Plus the CLI at `cli/` (`nixfleet-cli`) which has its own integration tests.

## How to run

The canonical entry point is `nix run .#validate -- --all` (see
[Testing Overview](overview.md)). For faster Rust-only iteration:

```sh
nix run .#validate -- --rust
```

That runs `cargo test --workspace` + `cargo clippy --workspace
--all-targets -- -D warnings` + `nix build` of every Rust package (the
sandboxed test run), in order. Use this over raw `cargo test` so clippy
and the sandbox-build check stay in the loop.

When you need to drill into a specific failure after `--rust` has
already surfaced it:

```sh
nix develop --command cargo test -p nixfleet-control-plane --test route_coverage
nix develop --command cargo test -p nixfleet-cli --test subcommand_coverage
nix develop --command cargo test -p nixfleet-agent --test run_loop_scenarios \
    poll_hint_shortens_next_interval
```

The Rust toolchain (`cargo`, `rustc`, `clippy`, `rustfmt`,
`rust-analyzer`) is pinned in the dev shell.

## Unit tests (in-file `#[cfg(test)] mod tests`)

Each Rust module has its own unit tests exercising pure logic without
HTTP / DB / filesystem / network.

### `nixfleet-control-plane`

| Module | Tested logic |
|---|---|
| `auth.rs` | API key SHA-256 hashing, role matrix (`admin`/`deploy`/`readonly`), bearer token parsing, role check predicates |
| `db.rs` | Every persistence method: register machine, insert report, generations table, releases + release entries, rollout batches, lifecycle filter, tag join (`machine_tags`), `get_recent_reports` deterministic tiebreaker, migrations idempotency |
| `metrics.rs` | Counter/gauge updates, Prometheus text rendering |
| `state.rs` | `FleetState` hydration from DB on startup, in-memory machine inventory, `poll_hint` propagation |
| `tls.rs` | Server/client cert loading, rustls `ServerConfig` / `ClientConfig` builder |
| `rollout/batch.rs` | Batch building from strategy (`all_at_once`, `canary`, `staged`), `batch_sizes` parsing (absolute N and percent), randomization determinism |
| `rollout/executor.rs` | `parse_threshold` (absolute + percent), `tick_for_tests` doc-hidden shim for deterministic single-tick advancement |

### `nixfleet-agent`

| Module | Tested logic |
|---|---|
| `comms.rs` | Report payload serialization, HTTP client builder with mTLS |
| `config.rs` | Default config (e.g., `dry_run = false`, `tags = []`), CLI arg parsing |
| `nix.rs` | `/run/current-system` symlink resolution, store-path parsing, generation hash extraction |
| `store.rs` | SQLite state DB: get/set `current_generation`, `log_check`, `log_error`, cleanup |
| `tls.rs` | Client cert/key loading, fleet CA trust |

### `nixfleet-types`

| Module | Tested logic |
|---|---|
| `lib.rs` | Serde round-trips for all wire types |
| `health.rs` | `HealthReport` + `HealthCheckResult` serialization |
| `release.rs` | `Release` / `ReleaseEntry` serde |
| `rollout.rs` | `RolloutStatus`, `RolloutStrategy`, `OnFailure` enum serde |

## Integration tests (Phase 3 scenarios)

Integration tests live in `control-plane/tests/*.rs` and `cli/tests/*.rs`.
Every file is an independent test binary — `cargo test` spawns one binary
per file.

### Shared harness

Every Phase 3 scenario file imports `control-plane/tests/harness.rs` via
a `#[path = "harness.rs"] mod harness;` sibling include. The harness
provides:

| Helper | Purpose |
|---|---|
| `spawn_cp()` / `spawn_cp_at(path)` | Boot an in-process CP bound to a temp directory with pre-seeded admin / deploy / readonly API keys. Returns a `SpawnedCp` handle with `.state`, `.db`, `.api_key`, `.base_url`, etc. |
| `register_machine(cp, id, tags)` | POST `/api/v1/machines/{id}/register` with the seeded admin key. |
| `create_release(cp, entries)` | POST `/api/v1/releases` with per-host store paths. Returns the release id. |
| `create_rollout_for_tag(cp, release_id, tag, strategy, batch_sizes, threshold, on_failure, health_timeout)` | POST `/api/v1/rollouts`. Returns the rollout id. |
| `fake_agent_report(cp, machine_id, generation, success, message, tags)` | POST `/api/v1/machines/{id}/report` as an agent. |
| `insert_health_report(machine_id, results, all_passed)` | Raw DB insert bypass for when you need to seed health state without going through the HTTP layer. |
| `tick_once(cp)` | Drive a single executor tick deterministically via `executor::test_support::tick_for_tests`. Replaces the production 2 s `tokio::time::interval`. |
| `wait_rollout_status(cp, rollout_id, status, per_tick_sleep)` | Repeatedly tick + check until the rollout reaches `status` or times out. |

Constants: `TEST_API_KEY`, `TEST_DEPLOY_KEY`, `TEST_READONLY_KEY` are
the three pre-seeded role keys.

### Scenario files — control-plane

| File | Covers |
|---|---|
| `agent_integration.rs` | Pre-Phase-3 baseline agent ↔ CP end-to-end tests (28 functions). |
| `release_scenarios.rs` | R3 push-hook invocation, R4 release list pagination, R5 referenced release delete → 409, R6 orphan release delete → 204. |
| `deploy_scenarios.rs` | D2 canary strategy happy path, D3 staged strategy happy path. |
| `failure_scenarios.rs` | F4 `get_recent_reports` sub-second tiebreaker determinism, F5 `failure_threshold = "30%"` pauses on 4 of 10, F6 CP restart mid-rollout resumes from DB (ADR 010). |
| `hydration_scenarios.rs` | H1 `FleetState` hydration on CP startup from a non-empty DB. |
| `rollback_scenarios.rs` | RB3 rollback via CP API, RB4 rollback edge cases. |
| `polling_scenarios.rs` | P1 CP emits `poll_hint = 5` during active rollouts, P2 `poll_hint` clears after rollout completes. Note: the agent-side loop honouring the hint is deferred. |
| `machine_scenarios.rs` | M1 `get_machines_by_tags` lifecycle filter (decommissioned agents excluded), M2 tag propagation via health reports. |
| `auth_scenarios.rs` | A1 bootstrap conflict (409), A2 role matrix (admin/deploy/readonly access patterns), A4 unauthenticated requests rejected. |
| `audit_scenarios.rs` | AU1 audit log writes for rollout lifecycle events, AU2 CSV-injection escaping for untrusted actor values. |
| `metrics_scenarios.rs` | ME1 `/metrics` exposure + counter updates, ME2 gauge accuracy. |
| `infra_scenarios.rs` | I1 migrations idempotency (run V1..V6 twice, no errors), Phase-2-archived tables absent. |
| `cn_validation_scenarios.rs` | Phase 4 mTLS CN validation middleware: no extension / empty extension / matching CN / mismatched CN. |
| `route_coverage.rs` | Phase 4 § 5 #2 — happy + error + auth coverage for every admin route, grouped by family via section headers (machines / rollouts / releases / audit+bootstrap+public). ~50 tests. |
| `executor_transition_scenarios.rs` | Phase 4 § 5 #4 — explicit positive + negative coverage for every executor state transition (Created→Running, Running→Completed, Running→Cancelled, Paused→Cancelled, Cancelled terminal). |
| `auth_matrix.rs` | Phase 4 § 5 #8 — role × endpoint auth matrix for every admin route + invalid bearer + missing prefix. |
| `migrations_scenarios.rs` | Phase 4 § 5 #9 — fresh DB schema shape, refinery_schema_history exists, idempotent on second migrate, every expected table is queryable. |

### Scenario files — cli

| File | Covers |
|---|---|
| `release_hook_scenarios.rs` | CLI-side of R3 — `release create --push-hook "..."` expands `{}` to the store path and runs the hook under `sh -c`. |
| `rollback_cli_scenarios.rs` | RB2 CLI dispatch — `nixfleet rollback --host <h> --generation <g>` constructs the right SSH invocation. |
| `config_scenarios.rs` | I2 CLI/credentials/file precedence + I2 env-var precedence (`NIXFLEET_*` overrides credentials, lose to CLI flags) + I3 `HOSTNAME` fallback path. |
| `subcommand_coverage.rs` | Phase 4 § 5 #1 — direct CLI test for every leaf subcommand (init, bootstrap, status, host add, machines list/untag/register, rollout list/status/cancel, release list/show/diff). |
| `release_delete_scenarios.rs` | Phase 4 — `nixfleet release delete` CLI dispatch (204 → exit 0, 409 → exit 1, 404 → exit 1). |

## Tests deliberately NOT in Rust

- Everything that needs a real systemd unit (`nixfleet-agent.service`,
  `harmonia.service`, `sshd`) — those are VM tests.
- Anything that needs a real `/run/current-system` symlink to resolve — the
  agent's `nix::current_generation()` returns an `unwrap_or_default()` at
  the call site, so the path is testable in VMs only.
- End-to-end CLI + real nix builds — those are VM tests
  (`vm-fleet-release`, `vm-fleet-deploy-ssh`, `vm-fleet-rollback-ssh`).

## Known gaps

(All previously listed gaps were closed in Phase 4. New gaps surfacing
during operation should be added here and tracked in `TODO.md`.)

## Coverage measurement

NixFleet measures Rust coverage with `cargo llvm-cov` on demand. The
core hardening cycle (closed 2026-04-11) explicitly declined to record
a one-shot baseline snapshot — an orphaned number from a single point
in time is theater without a concrete change to compare against.

The useful measurement is **"coverage delta for the code you just
touched"**, not "total workspace coverage at an arbitrary date."

### When to run

- Before merging a non-trivial Rust change, to confirm the new code is
  covered by at least one test path.
- Before a release, to spot-check any module whose coverage has drifted.
- When investigating a regression, to see whether the failing path had
  test coverage prior to the break.

### How to run

```sh
cargo install cargo-llvm-cov  # once per toolchain
cargo llvm-cov --workspace --html
# Open target/llvm-cov/html/index.html for the per-crate breakdown.

# Or on a specific crate / test target:
cargo llvm-cov --package nixfleet-control-plane --html
cargo llvm-cov --package nixfleet-agent --test run_loop_scenarios --html

# Diff against a baseline (e.g. pre-refactor):
cargo llvm-cov --workspace --summary-only > /tmp/post.txt
git checkout main
cargo llvm-cov --workspace --summary-only > /tmp/pre.txt
diff /tmp/pre.txt /tmp/post.txt
```

The html output is the primary operator experience. `--summary-only`
produces a text table suitable for piping into diff tools.

### What's not here

There is no persistent coverage percentage in this document. The spec
for the core hardening cycle called the baseline "not a hard gate —
measured, not enforced," and Phase 4 decided that a static snapshot in
docs has no downstream consumer. If a future cycle wants to establish
a persistent baseline (e.g. as a CI regression gate), the tooling
above is ready.

## Adding a new Rust scenario

1. Create `control-plane/tests/<domain>_scenarios.rs` or
   `cli/tests/<domain>_scenarios.rs`.
2. Add the harness sibling include at the top:

   ```rust
   #[path = "harness.rs"]
   mod harness;

   use harness::*;
   ```

3. Write `#[tokio::test]` functions. Use the `spawn_cp` / `register_machine`
   / `create_release` / `tick_once` helpers so your scenario doesn't fight
   the executor's wall-clock interval.
4. Run `cargo test -p nixfleet-control-plane --test <file>` to iterate.
5. If the scenario uncovers a product bug, fix the bug rather than
   adapting the test around it. See the test-vs-component debugging
   rule: when a test fails, first determine whether the test or the
   tested component needs fixing, before choosing a fix. Prefer
   root-cause fixes.
