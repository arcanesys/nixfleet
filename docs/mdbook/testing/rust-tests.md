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

## Integration tests (scenario files)

Integration tests live in `control-plane/tests/*_scenarios.rs`,
`control-plane/tests/route_coverage.rs`, and `cli/tests/*_scenarios.rs`.
Every file is an independent test binary — `cargo test` spawns one
binary per file.

### Shared harness

Every scenario file imports `control-plane/tests/harness.rs` via a
`#[path = "harness.rs"] mod harness;` sibling include. The harness
provides:

| Helper | Purpose |
|---|---|
| `spawn_cp()` / `spawn_cp_at(path)` | Boot an in-process CP bound to a temp directory with pre-seeded admin / deploy / readonly API keys. Returns a `Cp` handle with `.db`, `.fleet`, `.admin`, `.base`, `.db_path`. |
| `spawn_cp_with_rollout(store_path)` | Canonical "1 machine, 1 release, 1 all-at-once rollout, zero-tolerance, pause-on-failure" fixture. Returns `(cp, release_id, rollout_id)`. |
| `register_machine(cp, id, tags)` | Register a machine directly via DB + fleet state (bypasses HTTP for setup speed). |
| `create_release(cp, entries)` | `POST /api/v1/releases`; returns the release id. |
| `create_rollout_for_tag(cp, release_id, tag, strategy, batch_sizes, threshold, on_failure, health_timeout)` | `POST /api/v1/rollouts`; returns the rollout id. |
| `fake_agent_report(cp, machine_id, generation, success, message, tags)` | `POST /api/v1/machines/{id}/report` as an agent. |
| `agent_reports_health(cp, machine_id, store_path, healthy)` | Paired helper that emits both a `fake_agent_report` and an `insert_health_report` — the executor's generation gate and batch health gate read different tables, so almost every failure / recovery scenario needs both together. |
| `assert_status(builder, expected)` | One-line replacement for the `let resp = ...; .send().await; assert_eq!(resp.status(), N)` triple used across `route_coverage.rs`. |
| `tick_once(cp)` | Drive a single executor tick deterministically via `executor::test_support::tick_for_tests`. Replaces the production 2s `tokio::time::interval`. |
| `wait_rollout_status(cp, rollout_id, want, within)` | Poll `GET /rollouts/{id}` until status matches or the deadline elapses. |

Constants: `TEST_API_KEY`, `TEST_DEPLOY_KEY`, `TEST_READONLY_KEY` are
the three pre-seeded role keys.

### Scenario files — control-plane

| File | Covers |
|---|---|
| `release_scenarios.rs` | R3 push-hook invocation, R4 release list pagination, R5 referenced release delete → 409, R6 orphan release delete → 204. |
| `deploy_scenarios.rs` | D2 canary strategy happy path, D3 staged strategy happy path. |
| `failure_scenarios.rs` | Generation-gate filters stale-gen reports, `failure_threshold = "30%"` pauses on 4 of 10, resume does not re-flip on a stale pre-resume report, Paused → Cancelled via operator cancel. |
| `hydration_scenarios.rs` | CP restart mid-rollout resumes from DB (ADR 010) — cp1 stages a rollout, cp2 hydrates from the shared SQLite file and drives it to completion, proving `FleetState` is re-queried per tick. |
| `rollback_scenarios.rs` | Rollback via CP API: redeploy an old release as a forward rollback; original forward rollout stays Completed (history preserved). |
| `polling_scenarios.rs` | `poll_hint = 5` present when a machine is in an active rollout, absent when idle. |
| `machine_scenarios.rs` | M1 lifecycle filter (decommissioned excluded from rollout targets), M2 tag propagation via health reports, M3 direct desired-gen ↔ report cycle, M4 `success=false` → `system_state=error`, M5 multi-machine desired-gen isolation, M6 `Pending → Active` auto-transition, M7 `Active ↔ Maintenance` round trip. |
| `auth_scenarios.rs` | Bootstrap 409 after first key, anonymous admin route → 401, public `/health` stays open, readonly/deploy role enforcement on `POST /rollouts` and READ_ONLY on `GET /releases+/rollouts`, bearer-token shape errors (invalid token / missing `Bearer ` prefix → 401). |
| `audit_scenarios.rs` | Audit log writes for every mutating route + CSV-injection escaping for untrusted detail fields. |
| `metrics_scenarios.rs` | `/metrics` exposes every CP-side metric after a real rollout cycle, and the HTTP middleware counter increments per normalized path. |
| `cn_validation_scenarios.rs` | mTLS CN validation middleware: no extension / empty extension / matching CN / mismatched CN (defense in depth above the CA boundary). |
| `route_coverage.rs` | Happy + error + auth coverage for every admin route, grouped by family via section headers (machines / rollouts / releases / audit+bootstrap+public). ~50 tests. |
| `migrations_scenarios.rs` | Fresh DB schema shape, `refinery_schema_history` exists, idempotent on second migrate, every expected table is queryable. |

### Scenario files — cli

| File | Covers |
|---|---|
| `release_hook_scenarios.rs` | `release create --push-hook "..."` expands `{}` to the store path and runs the hook under `sh -c`. |
| `rollback_cli_scenarios.rs` | `nixfleet rollback --host <h> --generation <g>` constructs the right SSH invocation. |
| `config_scenarios.rs` | CLI/credentials/file precedence + env-var precedence (`NIXFLEET_*` overrides credentials, loses to CLI flags) + `HOSTNAME` fallback path. |
| `subcommand_coverage.rs` | Direct CLI test for every leaf subcommand (init, bootstrap, status, host add, machines list/untag/register, rollout list/status/cancel, release list/show/diff). |
| `release_delete_scenarios.rs` | `nixfleet release delete` CLI dispatch (204 → exit 0, 409 → exit 1, 404 → exit 1). |

## Tests deliberately NOT in Rust

- Everything that needs a real systemd unit (`nixfleet-agent.service`,
  `harmonia.service`, `sshd`) — those are VM tests.
- Anything that needs a real `/run/current-system` symlink to resolve — the
  agent's `nix::current_generation()` returns an `unwrap_or_default()` at
  the call site, so the path is testable in VMs only.
- End-to-end CLI + real nix builds — those are VM tests
  (`vm-fleet-release`, `vm-fleet-deploy-ssh`, `vm-fleet-rollback-ssh`).

## Known gaps

New gaps surfacing during operation should be added here and tracked
in `TODO.md`.

## Coverage measurement

NixFleet measures Rust coverage with `cargo llvm-cov` on demand. We
deliberately do not record a one-shot baseline snapshot — an orphaned
number from a single point in time is theater without a concrete
change to compare against.

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

# Diff the branch under review against main:
cargo llvm-cov --workspace --summary-only > /tmp/branch.txt
git checkout main
cargo llvm-cov --workspace --summary-only > /tmp/main.txt
diff /tmp/main.txt /tmp/branch.txt
```

The html output is the primary operator experience. `--summary-only`
produces a text table suitable for piping into diff tools.

### What's not here

There is no persistent coverage percentage in this document — a
static snapshot has no downstream consumer. If a future change wants
to establish a persistent baseline (e.g. as a CI regression gate),
the tooling above is ready.

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
