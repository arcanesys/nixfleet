# ADR 009 — Core Hardening Audit (Phase 1 Decision Table)

**Date:** 2026-04-10
**Status:** Draft — pending row-by-row user approval
**Cycle:** Core hardening (spec: `docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md`)

## Context

The release-abstraction cycle (PR #28) revealed that nixfleet's Rust core has half-working features, tautological tests, and untested critical paths. This audit is Phase 1 of the core hardening cycle: it inventories every feature and infrastructure layer in the Rust core and assigns a verdict to each.

## Verdict key

- **Keep** — already tested or trivially correct; nothing to do
- **Test** — write a real integration test that would fail if the feature broke
- **Delete** — archive branch + remove (code preserved at `archive/<feature>` branch)
- **Trim** — remove dead subset, keep the rest
- **Fix** — trivial bug fix + test (non-trivial fixes get deferred to a new cycle)

## Categories

1. CLI subcommands
2. Control-plane HTTP endpoints
3. Agent behaviors
4. Executor features
5. Shared types
6. Dependencies
7. Tautological tests
8. `#[allow(dead_code)]` escapes
9. Background tasks
10. State hydration
11. Prometheus metrics emission
12. Audit logging
13. TLS / mTLS
14. Auth middleware
15. DB migrations
16. Config loading
17. Agent health check subsystem
18. Nix interaction layer
19. Comms layer

## Category 1: CLI subcommands

**Enumeration confirmed via `cli/src/main.rs` + per-subcommand files.** 27 subcommands total, **0% test coverage** (no integration tests, no unit tests). All handlers exist, parse args correctly, and make HTTP calls — but no behavior test would catch a broken call.

| Subcommand | File | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|
| `init` | `cli/src/main.rs:201` → `config::write_config_file` | None | Test | Core UX, must work on first run |
| `bootstrap` | `cli/src/main.rs:191` → inline `bootstrap()` | None | Test | Security-critical, creates first admin key |
| `status` | `cli/src/status.rs` | None | Test | Read path, low effort |
| `machines list` | `cli/src/machines.rs` | None | Test | Read path |
| `machines register` | `cli/src/machines.rs` | None | Test | Write path |
| `machines tag` | `cli/src/machines.rs` | None | **Trim** | Redundant with agent auto-sync (tags flow from NixOS config → health report) |
| `machines untag` | `cli/src/machines.rs` | None | Test | Only path to remove stale tags |
| `release create` | `cli/src/release.rs` | Indirect via PR #28 validation | Keep (indirect) → Test | End-to-end validated live; still needs unit test |
| `release list` | `cli/src/release.rs` | None | Test | Fill gap |
| `release show` | `cli/src/release.rs` | None | Test | Fill gap |
| `release diff` | `cli/src/release.rs` | None | Test | Core UX |
| `rollout list` | `cli/src/rollout.rs` | None | Test | Fill gap |
| `rollout status` | `cli/src/rollout.rs` | None | Test | Events timeline |
| `rollout resume` | `cli/src/rollout.rs` | `vm-fleet.nix` covers resume path | Keep | Integration tested |
| `rollout cancel` | `cli/src/rollout.rs` | None | Test | Untested critical path |
| `policy create` / `list` / `get` / `update` / `delete` | `cli/src/policy.rs` | None | **Delete or Test — user decides** | Ergonomic preset layer; real operator story unclear |
| `schedule list` / `cancel` | `cli/src/schedule.rs` | None | **Delete or Test — user decides** | Deferred rollouts without a real operator story |
| `deploy` (CP path) | `cli/src/deploy.rs` | `vm-fleet.nix` happy path | Keep | Integration tested |
| `deploy --ssh` | `cli/src/deploy.rs` | None | Test | Bypass path, must work without CP |
| `rollback` (SSH-only) | `cli/src/main.rs:884` inline | None | Test | Core rollback path; explicitly documents post-PR #28 SSH-only constraint |
| `host add` | `cli/src/host.rs` | None | Test | Registers machine in CP fleet |
| `host provision` | `cli/src/host.rs` | None | **Delete or Test — user decides** | Thin wrapper around nixos-anywhere; value-add is a question |

**Key observation:** CLI is the operator's primary interface and has 0% coverage. Phase 3 scenarios + Phase 4 per-subcommand checklist together must get this to full coverage.

## Category 2: Control-plane HTTP endpoints

**31 routes total** (2 agent mTLS, 26 admin API key, 1 bootstrap, 2 public). **Only 7 routes have any tests**, all in `control-plane/tests/agent_integration.rs`, and all happy-path only. **`set-generation` confirmed removed** in PR #28 (not in table). Test counts below are `(happy/error/auth)`.

| Route | Method | Auth | Handler | Tests (h/e/a) | Proposed verdict | Rationale |
|---|---|---|---|---|---|---|
| `/api/v1/machines/{id}/desired-generation` | GET | mTLS | `routes::get_desired_generation` | 1/1/0 | Test | mTLS-missing negative needed |
| `/api/v1/machines/{id}/report` | POST | mTLS | `routes::post_report` | 3/1/0 | Test | Bad report shape + auth-missing |
| `/api/v1/machines` | GET | API key | `routes::list_machines` | 3/0/1 | Test | Error paths untested |
| `/api/v1/machines/{id}/register` | POST | API key | `routes::register_machine` | 3/0/0 | Test | Auth-missing untested |
| `/api/v1/machines/{id}/lifecycle` | PATCH | API key | `routes::update_lifecycle` | 2/1/0 | Test | State transition matrix |
| `/api/v1/machines/{id}/tags` | POST | API key | `routes::set_tags` | 0/0/0 | **Trim** | Redundant with agent auto-sync |
| `/api/v1/machines/{id}/tags/{tag}` | DELETE | API key | `routes::remove_tag` | 0/0/0 | Test | Only way to remove stale tags |
| `/api/v1/rollouts` | POST | API key | `rollout::routes::create_rollout` | 0/0/0 | Test | Completely untested |
| `/api/v1/rollouts` | GET | API key | `rollout::routes::list_rollouts` | 0/0/0 | Test | Pagination untested |
| `/api/v1/rollouts/{id}` | GET | API key | `rollout::routes::get_rollout` | 0/0/0 | Test | Events timeline untested |
| `/api/v1/rollouts/{id}/resume` | POST | API key | `rollout::routes::resume_rollout` | 0/0/0 | Test | `vm-fleet.nix` covers happy path indirectly; need direct test |
| `/api/v1/rollouts/{id}/cancel` | POST | API key | `rollout::routes::cancel_rollout` | 0/0/0 | Test | Cancel untested |
| `/api/v1/policies` (POST/GET) | POST/GET | API key | `rollout::policy::*` | 0/0/0 | **Delete or Test — user decides** | Depends on Cat 1 policy decision |
| `/api/v1/policies/{name}` (GET/PUT/DELETE) | GET/PUT/DELETE | API key | `rollout::policy::*` | 0/0/0 | **Delete or Test — user decides** | Same |
| `/api/v1/schedules` (POST/GET) | POST/GET | API key | `rollout::schedule::*` | 0/0/0 | **Delete or Test — user decides** | Depends on Cat 1 schedule decision |
| `/api/v1/schedules/{id}` (GET) | GET | API key | `rollout::schedule::get_schedule` | 0/0/0 | **Delete or Test — user decides** | Same |
| `/api/v1/schedules/{id}/cancel` | POST | API key | `rollout::schedule::cancel_schedule` | 0/0/0 | **Delete or Test — user decides** | Same |
| `/api/v1/releases` | POST | API key | `release::routes::create_release` | 0/0/0 | Test | Validation + audit write |
| `/api/v1/releases` | GET | API key | `release::routes::list_releases` | 0/0/0 | Test | Pagination |
| `/api/v1/releases/{id}` | GET | API key | `release::routes::get_release` | 0/0/0 | Test | 404 path |
| `/api/v1/releases/{id}` | DELETE | API key | `release::routes::delete_release` | 0/0/0 | Test | 409-when-referenced critical |
| `/api/v1/releases/{id}/diff/{other}` | GET | API key | `release::routes::diff_releases` | 0/0/0 | Test | Core UX |
| `/api/v1/audit` | GET | API key | `audit::list_audit_events` | 1/0/0 | Test | Filtering untested |
| `/api/v1/audit/export` | GET | API key | `audit::export_audit_csv` | 1/0/0 | Test | CSV shape + injection protection (already protected via `escape_csv_field`) |
| `/api/v1/keys/bootstrap` | POST | none | `routes::bootstrap_api_key` | 0/0/0 | Test | **Critical gap**: 409-on-re-run untested, first-key-is-admin not asserted |
| `/health` | GET | none | inline | 2/0/0 | Keep | Trivial, already tested |
| `/metrics` | GET | none | `metrics::metrics_handler` | 0/0/0 | Test | Overlaps Category 11 |

**Observation:** 24 of 31 routes (77%) have zero test coverage. The **bootstrap endpoint** in particular — the only unauthenticated POST — has no test for the 409-conflict-on-re-run invariant, which is the core of its security story.

## Category 3: Agent behaviors

Agent code at `agent/src/main.rs` + supporting modules. Every behavior is **"validated live" via `vm-fleet.nix`** but **none has a direct unit or harness test**. The state-machine bug (fixed in PR #28) proves this layer is under-tested.

| Behavior | Code location | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|
| Poll loop cadence | `agent/src/main.rs:136-187` (`run_deploy_cycle` + tokio select!) | `vm-fleet.nix` integration only | Test | Main-loop bug hid here for months |
| Deploy: Check phase | `agent/src/main.rs:244-274` | Integration only | Test | Cycle completeness |
| Deploy: Fetch phase | `agent/src/main.rs:282-296` → `nix::fetch_closure` | Integration only | Test | Failure paths critical |
| Deploy: Apply phase | `agent/src/main.rs:306-314` → `nix::apply_generation` | Integration only | Test | Sandboxing bugs hid here |
| Deploy: Verify phase (post-apply health) | `agent/src/main.rs:316-336` | `vm-fleet.nix` gate on db-01 | Keep (integration-only) → Test | Gate is tested; health check hookup needs direct test |
| Deploy: Report phase | `agent/src/main.rs:374-390` → `comms::post_report` | Integration only | Test | Generation gate depends on this |
| Health reporting (periodic) | `agent/src/main.rs:210-223` (60s interval) | Integration only | Test | Tag sync ride-along |
| Automatic rollback on apply failure | `agent/src/main.rs:308-312` | Integration: `vm-fleet.nix` apply-fail scenario | Test | Direct test for error path |
| Automatic rollback on health gate failure | `agent/src/main.rs:318-335` | Integration: `vm-fleet.nix` db-01 unhealthy | Keep (integration-only) → Test | Direct test desirable |
| Retry on poll failure | `agent/src/main.rs:248-258` + `:179-182` (`retry_interval` rebuild) | Integration only | Test | Bootstrap race handling |
| Tag auto-sync via health report | `agent/src/main.rs:210-223` → `comms::post_report` | None | Test | Self-managing invariant; bug would be silent |
| Metrics endpoint (port binding, emission) | `agent/src/metrics.rs:7-12`, bound at `agent/src/main.rs:126-128` | None | Test | Overlaps Category 11 |
| `nix path-info` verification (no cache) | `agent/src/nix.rs:19-45` (fallback branch) | Integration only | Test | Cache-less verification path |
| Adaptive polling via `poll_hint` | `agent/src/main.rs:144-154` + `:169-184` | None | Test | `poll_hint` propagation from CP |

**Verdict summary:** all behaviors are currently `Test` — every single row needs a direct test that would fail if the behavior broke. The only reason any of these would be `Keep` is if a Phase 3 scenario already directly exercises them (which none do today).

## Category 4: Executor features

Rollout executor at `control-plane/src/rollout/executor.rs` + `batch.rs` + `policy.rs` + `schedule.rs`. Unit tests exist for **batch sizing** (`batch.rs:74-129`, 7 tests) and **threshold parsing** (`executor.rs:679-694`). Nothing else has a direct test.

| Feature | Code | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|
| Canary strategy | `batch.rs:64` + `executor.rs:176` | `batch.rs::test_canary_batch_sizes`, `test_build_batches_canary_20_machines`; `vm-fleet.nix` web tag | Keep | Unit + integration |
| Staged strategy | `batch.rs:66-69` + `executor.rs:177` | `batch.rs::test_build_batches_staged` | Test | Executor path untested (unit test covers batching only) |
| All-at-once strategy | `batch.rs:65` + `executor.rs:178` | `batch.rs::test_build_batches_all_at_once`; `vm-fleet.nix` db tag (pause path) | Test | Happy path untested directly |
| Batch creation & sizing | `batch.rs:4-47` (`build_batches`, `parse_batch_size`) | 7 unit tests | Keep | Well-covered |
| Batch deployment | `executor.rs:308-384` (`deploy_batch`) | Indirect only | Test | Sets desired + stores previous_gens — core write path |
| Batch health evaluation | `executor.rs:387-614` (`evaluate_batch`) | Indirect | Test | Complex: generation gate + timeout + threshold |
| Health gate (generation match) | `executor.rs:418-420` | None | Test | **PR #28 critical bug source** — must have direct test |
| Health timeout | `executor.rs:461-497` | None | Test | Time-based path |
| Failure threshold | `executor.rs:9-20` + `:500` | `test_parse_threshold_*` (parser only) | Test | Evaluation path untested |
| `on_failure=pause` | `executor.rs:553-575` | `vm-fleet.nix` db tag | Keep (integration) → Test | Direct test for state transition |
| `on_failure=revert` | `executor.rs:576-598` + `:626-672` (`revert_completed_batches`) | None | Test | Per-machine revert via `previous_generations` untested |
| Release entry lookup | `executor.rs:289-294` (`get_release_entries`, `entry_map`) | None | Test | PR #28 addition, invariant-bearing |
| Per-machine `previous_generations` | `executor.rs:323-350` | None | Test | Heterogeneous revert — phase 3 scenario F2 |
| Policy reference resolution | `executor.rs:64-115` (via `trigger_due_schedules`) | None | **Delete or Test — depends on Cat 1** | Tied to policy subsystem decision |
| Schedule processing | `executor.rs:53-249` (`trigger_due_schedules`) | None | **Delete or Test — depends on Cat 1** | Tied to schedule subsystem decision |
| Rollout completion | `executor.rs:264-284` | Indirect | Test | Terminal state transition |

**Note on updated metrics:** `ROLLOUTS_ACTIVE` (`executor.rs:622`) and `ROLLOUTS_TOTAL` (completed/paused/failed at `:281`, `:573`, `:596`) are emitted from the executor — tested under Category 11.

## Category 5: Shared types

`shared/` = `nixfleet-types`. External ref counts are from grep across `agent/`, `control-plane/`, `cli/` (excluding `shared/src/` itself).

| Type | File | External refs | Dead variants | Proposed verdict | Rationale |
|---|---|---|---|---|---|
| `MachineLifecycle` | `lib.rs` | 11 | none | Keep | Core state machine |
| `DesiredGeneration` | `lib.rs` | 18 | — | Keep | Includes `poll_hint` |
| `Report` | `lib.rs` | 28 | — | Keep | Core agent→CP type |
| `MachineStatus` | `lib.rs` | 11 | — | Keep | Inventory view |
| `AuditEvent` | `lib.rs` | 5 | — | Keep | Compliance |
| `RolloutStrategy` | `rollout.rs` | 27 | none | Keep | Core type |
| `OnFailure` | `rollout.rs` | 13 | none | Keep | Core type |
| `RolloutStatus` | `rollout.rs` | 2 (+`is_active`, `from_str_lc`) | String-round-tripped, not pattern-matched externally | Keep | Serialized via string in DB — refs are low but the type is load-bearing |
| `BatchStatus` | `rollout.rs` | 6 | none | Keep | |
| `MachineHealthStatus` | `rollout.rs` | 5 | **`TimedOut`, `RolledBack` — 0 external refs** | **Trim** | Two dead variants — remove |
| `RolloutTarget` | `rollout.rs` | 12 | none | Keep | |
| `RolloutPolicy` | `rollout.rs` | 10 | — | **Delete — depends on Cat 1** | Tied to policy subsystem |
| `PolicyRequest` | `rollout.rs` | 8 | — | **Delete — depends on Cat 1** | Same |
| `RolloutEvent` | `rollout.rs` | 7 | — | Keep | Timeline |
| `ScheduleStatus` | `rollout.rs` | 3 | none | **Delete — depends on Cat 1** | |
| `ScheduledRollout` | `rollout.rs` | 4 | — | **Delete — depends on Cat 1** | |
| `CreateScheduleRequest` | `rollout.rs` | 4 | — | **Delete — depends on Cat 1** | |
| `CreateRolloutRequest` | `rollout.rs` | 6 | — | Keep | |
| `CreateRolloutResponse` | `rollout.rs` | 3 | — | Keep | |
| `BatchSummary` | `rollout.rs` | 3 | — | Keep | |
| `BatchDetail` | `rollout.rs` | 2 | — | Keep | |
| `RolloutDetail` | `rollout.rs` | 2 | — | Keep | |
| `HealthCheckResult` | `health.rs` | 25 | none (Pass/Fail both used) | Keep | |
| `HealthReport` | `health.rs` | 6 | — | Keep | |
| `Release` | `release.rs` | 46 | — | Keep | PR #28 core |
| `ReleaseEntry` | `release.rs` | 10 | — | Keep | PR #28 core |
| `CreateReleaseRequest` | `release.rs` | 4 | — | Keep | |
| `CreateReleaseResponse` | `release.rs` | 5 | — | Keep | |
| `ReleaseDiff` | `release.rs` | 6 | — | Keep | |
| `ReleaseDiffEntry` | `release.rs` | 2 | — | Keep | |

**Dead variants to trim in Phase 2:**
- `MachineHealthStatus::TimedOut` — 0 external refs
- `MachineHealthStatus::RolledBack` — 0 external refs

(Both are defined in `shared/src/rollout.rs` but never pattern-matched outside the crate. They were API artifacts from earlier design iterations.)

## Category 6: Dependencies

**Tool output:** `cargo machete` not run (dev shell install deferred — manual inspection only). All verdicts below are from grep-based use-count inspection.

### Delete candidates

| Crate | Dependency | Uses in src/ | Proposed verdict | Rationale |
|---|---|---|---|---|
| agent | `async-trait` | **0** | **Delete** | Declared but never used in src |
| agent | `tempfile` | **0** | **Delete** | Dev-only convention; move to `[dev-dependencies]` or drop |
| control-plane | `tempfile` | **0** | **Delete** | Same |
| shared | `serde_json` | **0** | **Delete** | Declared in shared but unused at lib level |

### Keep (all crates)

All other dependencies have substantial usage (tokio 37 uses, axum 36, reqwest 35, serde/serde_json 12/117, clap 3, tracing 80, rusqlite 76, anyhow 64, chrono 29, metrics 40, etc.). See Phase 2 plan for the full per-dep grep table.

**Note on `clap` usage = 3 in agent/control-plane:** these are arg-parsing entry-point imports only (which is correct — the daemons don't need heavy CLI parsing). Keep as-is.

**Refinement in Phase 2:** run `cargo machete` inside `nix develop` (not globally) to reconfirm, then submit the deletions as a single commit.

## Category 7: Tautological tests

~22 tests identified across the workspace. **A tautological test is worse than no test** — it lights up green while the feature is broken. All verdicts are **Delete**. If the targeted behavior is important, a real test gets written under Category 4/11/etc.

| Test | File:line | Pattern | Proposed verdict | Rationale |
|---|---|---|---|---|
| `test_config_defaults` | `agent/src/config.rs:62` | Creates a `Duration::from_secs(300)` then asserts `.as_secs() >= 60` — tests nothing about config | Delete | No config loading involved |
| `test_config_poll_interval_at_minimum_boundary` | `agent/src/config.rs:70` | Asserts `60 >= 60` | Delete | Self-referential |
| `test_config_poll_interval_at_maximum_boundary` | `agent/src/config.rs:76` | Asserts `3600 <= 3600` | Delete | Self-referential |
| `test_config_clone` | `agent/src/config.rs:82` | Clones struct, asserts field equals | Delete | Tests `Clone` derive, not behavior |
| `test_parse_full_config` | `agent/src/health/config.rs:69` | Serde round-trip, asserts parsed == input | Delete | No behavior |
| `test_parse_empty_config` | `agent/src/health/config.rs:87` | Serde round-trip of `{}` | Delete | No behavior |
| `test_defaults_applied` | `agent/src/health/config.rs:96` | Asserts default function returns a hardcoded value | Delete | No behavior |
| `test_camel_case_from_nix` | `agent/src/health/config.rs:112` | Serde round-trip | Delete | No behavior |
| `test_report_serialization` | `shared/src/lib.rs:205` | Serde round-trip | Delete | No behavior |
| `test_report_failure_serialization` | `shared/src/lib.rs:224` | Serde round-trip | Delete | No behavior |
| `test_desired_generation_serialization_roundtrip` | `shared/src/lib.rs:69` | Serde round-trip | Delete | No behavior |
| `test_lifecycle_from_str` | `shared/src/lib.rs` | Parse + reverse, no input variation | Delete | Trivial enum roundtrip |
| `test_lifecycle_valid_transitions` | `shared/src/lib.rs` | Enum method with one case | Delete (or merge into real transition matrix test) | Single input, no variation |
| `test_normalize_static_paths` | `control-plane/src/metrics.rs` | Normalizes `/health` → `/health` | Delete | Identity function |
| `test_normalize_machine_id_paths` | `control-plane/src/metrics.rs` | Tests one machine-id path | Keep (marginal) → Test | Actual behavior but single-case; expand or delete |
| `test_hash_key_deterministic` | `control-plane/src/auth.rs` | Hash same input twice, assert equal | Delete | Tests SHA256 determinism (stdlib invariant) |
| `test_actor_identifier_apikey` | `control-plane/src/auth.rs` | Formats enum, asserts string | Delete | Tests `Display` impl |
| `test_canary_batch_sizes` | `control-plane/src/rollout/batch.rs` | Lookup table assertion | Keep | Fine — single simple thing it tests is correct |
| `test_build_batches_canary_20_machines` | `control-plane/src/rollout/batch.rs` | Builds batches for one fixed input | Keep | Real but thin — expand in Phase 4 |
| `test_migrate_is_idempotent` | `control-plane/src/db.rs:1382` | Name claims idempotency but only calls `migrate()` **once** | **Fix** (trivial) — call migrate twice, verify no error; then Keep | Misleading test; easy to fix |
| `test_set_and_get_desired_generation` | `control-plane/src/db.rs:1388` | CRUD round-trip | Delete (or replace with property test) | Tests rusqlite round-trip, not business logic |
| `test_register_machine` | `control-plane/src/db.rs` | CRUD round-trip | Delete (or replace) | Same |

**Summary:** ~18 to delete, 1 to fix (`test_migrate_is_idempotent`), 3 to keep as-is or lightly expand.

## Category 8: `#[allow(dead_code)]` escapes

**Surprise result:** 3 escapes found, **all 3 are false positives** (the fields are populated via serde from Nix-generated JSON or via CLI args, and the compiler can't see that usage).

| Location | Protected code | Actually used? | Proposed verdict | Rationale |
|---|---|---|---|---|
| `agent/src/health/config.rs:23` | `HttpConfig::interval: i64` | **Yes** — serde alias from Nix camelCase; asserted in tests at `:103`, `:114`, `:120` | Keep (+ comment) | Add a brief `// populated via serde alias from Nix config` comment so the escape is self-explanatory |
| `agent/src/health/config.rs:36` | `CommandConfig::interval: i64` | **Yes** — same pattern, asserted in tests at `:107`, `:120` | Keep (+ comment) | Same |
| `agent/src/config.rs:18` | `Config::db_path: String` | **Yes** — `agent/src/main.rs` reads `cli.db_path` and passes to `Store::new` | Keep (+ comment) | Add `// set from CLI arg in main.rs` comment |

**Verdict:** all 3 stay, with a comment annotation added in Phase 2 to make the escape self-documenting. **Zero actual dead code** under these escapes.

**Phase 2 action:** replace the bare `#[allow(dead_code)]` with an annotated form like:
```rust
#[allow(dead_code)] // populated via serde alias from Nix config
```

## Category 9: Background tasks

Only two `tokio::spawn` sites in the workspace — both in `control-plane/src/main.rs`.

| Task | Location | Purpose | Error handling | Lifetime | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|---|---|---|
| Rollout executor | `control-plane/src/main.rs:58` → `rollout::executor::spawn` | 2-second interval; processes running rollouts, triggers due schedules | Logs errors via `tracing`, continues loop | Infinite (process lifetime) | Indirect via `vm-fleet.nix` | Test | Direct test for error resilience + generation gate |
| Health report cleanup | `control-plane/src/main.rs:63` | Hourly interval; deletes health reports older than 24h | `tracing::warn` on error, continues loop | Infinite (process lifetime) | None | Test | Silent failure is a silent data-growth risk |

**No agent-side spawn sites.** The agent's deploy cycle is a single tokio select! loop, not multiple spawned tasks.

## Category 10: State hydration

`control-plane/src/state.rs::hydrate_from_db` is called at startup (`main.rs:55`). Hydrates 4 categories of in-memory state. Unit tests exist for the `FleetState` struct operations but **none for `hydrate_from_db` itself**.

| Item | Code | What's hydrated | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|---|
| `hydrate_from_db` (top-level) | `control-plane/src/state.rs:79-106` | async entrypoint | None | Test | F6 scenario in spec (CP restart mid-rollout) depends on this |
| Machines + lifecycle | `state.rs:79-86` → `db.list_machines()` | in-memory `FleetState::machines` | None | Test | |
| Machine tags | `state.rs:88-93` → `db.get_machine_tags()` | per-machine tag sync | None | Test | |
| Desired generations | `state.rs:96-104` → `db.list_desired_generations()` | `machine.desired_generation` | None | Test | |
| Active/paused rollouts | `state.rs:105-106` → `db.list_rollouts_by_status(...)` | **NOTE**: loaded but info-logged only, NOT stored in FleetState | None | Test or **Fix** | Executor re-queries these from DB — verify this is intentional, not a bug |
| `FleetState::new` + `get_or_create` | `state.rs:118-156` | N/A | `test_fleet_state_new_is_empty`, `test_get_or_create_inserts_new`, `test_get_or_create_returns_existing` | Keep | Adequate |

**Concern flagged:** the rollouts listing at `state.rs:105-106` is loaded and logged but not stored. If this was meant to warm FleetState caches, it's a bug. If it's just diagnostic logging, add a `// diagnostic only` comment in Phase 2. Investigate before writing the test.

## Category 11: Prometheus metrics emission

**Surprise result:** contrary to the Phase 1 plan's pre-filled guess, **`AGENT_STATE` is NOT dead**. PR #28 removed `agent/src/state.rs` (the state machine module) but kept the metric — it's still emitted from 17 transition sites in `agent/src/main.rs` tracking the linear deploy cycle (idle → checking → fetching → applying → verifying → reporting → rolling_back → idle).

13 metric constants, all actively emitted. Zero dead metrics.

| Metric | Declared in | Emission sites | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|---|
| `FLEET_SIZE` (`nixfleet_fleet_size`) | `shared/src/metrics.rs:7` | 1 (`control-plane/src/metrics.rs:80`) | None | Test | Core gauge |
| `MACHINES_BY_LIFECYCLE` (`nixfleet_machines_by_lifecycle`) | `shared/src/metrics.rs:8` | 5 (`control-plane/src/metrics.rs:107-113`) | None | Test | 5 variants all emitted |
| `MACHINE_LAST_SEEN_TIMESTAMP` (`..._seconds`) | `shared/src/metrics.rs:9` | 1 (`control-plane/src/metrics.rs:99-103`) | None | Test | |
| `HTTP_REQUESTS_TOTAL` (`nixfleet_http_requests_total`) | `shared/src/metrics.rs:10` | 1 (middleware `control-plane/src/metrics.rs:55-61`) | None | Test | Middleware correctness — path normalization already covered (Category 7 keeps `test_normalize_machine_id_paths`) |
| `HTTP_REQUEST_DURATION_SECONDS` | `shared/src/metrics.rs:11` | 1 (`control-plane/src/metrics.rs:63-68`) | None | Test | |
| `ROLLOUTS_ACTIVE` (`nixfleet_rollouts_active`) | `shared/src/metrics.rs:12` | 1 (`executor.rs:622`) | None | Test | |
| `ROLLOUTS_TOTAL` (`nixfleet_rollouts_total`) | `shared/src/metrics.rs:13` | 3 (`executor.rs:281` completed, `:573` paused, `:596` failed) | None | Test | Terminal-state counters |
| `AGENT_STATE` (`nixfleet_agent_state`) | `shared/src/metrics.rs:16` | **17** sites in `agent/src/main.rs` | None | Test | **Not dead** — still emitted after PR #28 |
| `AGENT_POLL_DURATION_SECONDS` | `shared/src/metrics.rs:17` | 1 (`agent/src/metrics.rs:23`) | None | Test | |
| `AGENT_LAST_POLL_TIMESTAMP_SECONDS` | `shared/src/metrics.rs:18` | 1 (`agent/src/metrics.rs:24`) | None | Test | |
| `AGENT_HEALTH_CHECK_DURATION_SECONDS` | `shared/src/metrics.rs:19-20` | 1 (`agent/src/metrics.rs:29-33`) | None | Test | |
| `AGENT_HEALTH_CHECK_STATUS` | `shared/src/metrics.rs:21` | 1 (`agent/src/metrics.rs:35-39`) | None | Test | |
| `AGENT_GENERATION_INFO` | `shared/src/metrics.rs:22` | 1 (`agent/src/metrics.rs:44`) | None | Test | |

**All 13 metrics need a `Test` verdict** — each should have an end-to-end assertion that the metric is present in `/metrics` output with the expected shape after the triggering action.

## Category 12: Audit logging

**15 of 16 mutating endpoints write audit entries**, with the bootstrap endpoint as the sole exception (audit write would be circular — it creates the first actor). Audit writes include actor / action / target / detail-JSON. CSV export is already injection-protected via `escape_csv_field`.

### Mutation → audit coverage

| Mutation | Handler | Audits written? | Proposed verdict | Rationale |
|---|---|---|---|---|
| Post machine report | `routes::post_report:119` | Yes (`"report"`) | Test | Assert audit row present after POST |
| Register machine | `routes::register_machine:271` | Yes (`"register"`) | Test | |
| Update lifecycle | `routes::update_lifecycle:348` | Yes (`"update_lifecycle"`) | Test | |
| Set tags | `routes::set_tags:397` | Yes (`"set_tags"`) | **Trim** | Tied to Cat 2 trim of `POST /machines/{id}/tags` |
| Remove tag | `routes::remove_tag:432` | Yes (`"remove_tag"`) | Test | |
| Create rollout | `rollout::routes::create_rollout:243` | Yes (`"rollout.created"`) | Test | |
| Resume rollout | `rollout::routes::resume_rollout:424` | Yes (`"rollout.resumed"`) | Test | |
| Cancel rollout | `rollout::routes::cancel_rollout:482` | Yes (`"rollout.cancelled"`) | Test | |
| Create/Update/Delete policy | `rollout::policy::*` | Yes | **Delete or Test — depends on Cat 1** | |
| Create/Cancel schedule | `rollout::schedule::*` | Yes | **Delete or Test — depends on Cat 1** | |
| Create release | `release::routes::create_release:59` | Yes (`"create_release"`) | Test | |
| Delete release | `release::routes::delete_release:234` | Yes (`"delete_release"`) | Test | |
| Bootstrap API key | `routes::bootstrap_api_key:442` | **No** (bootstrap is unauthenticated first-time setup) | **Fix** | Add audit write with actor=`"system:bootstrap"` so the first-key creation is traceable; then test |

### Audit query endpoints

| Endpoint | Current test | Proposed verdict | Rationale |
|---|---|---|---|
| `GET /api/v1/audit` | `test_audit_trail_on_register` (1 happy path) | Test | Filtering by actor/action/target untested |
| `GET /api/v1/audit/export` (CSV) | `test_audit_csv_export` (1 happy path) | Test | Injection protection already implemented (`escape_csv_field`); add negative test with `=HYPERLINK(...)` payload |

## Category 13: TLS / mTLS

Server side: `control-plane/src/tls.rs`. Client side: `agent/src/tls.rs`. End-to-end mTLS is exercised by `modules/tests/vm-fleet.nix` (fleet CA + CP server cert + 3 agent client certs) but no negative tests exist.

| Item | File | Purpose | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|---|
| `build_server_config` | `control-plane/src/tls.rs` | Load cert+key; if `client_ca_path` set, install `WebPkiClientVerifier` (mTLS required) | `test_build_server_config_missing_cert_fails`; `vm-fleet.nix` happy path | Test | Add negative tests: malformed cert PEM, malformed key PEM, missing CA file |
| `load_client_identity` | `agent/src/tls.rs` | Load PEM cert+key, build reqwest `Identity` | `test_load_missing_cert_fails`; `vm-fleet.nix` happy path | Test | Same negative tests (malformed key, etc.) |
| Agent mTLS enforcement (transport layer) | `control-plane/src/lib.rs` (agent_routes) | Agent routes require client cert when `client_ca_path` is set | `vm-fleet.nix` happy path only | Test | **Critical negative test**: connection without client cert → rejected |
| CA cert mismatch rejection | reqwest / rustls | Client cert signed by different CA → rejected | None | Test | Negative test |

**Integration coverage gap:** `vm-fleet.nix` proves the happy path works end-to-end, but it cannot prove that mTLS enforcement *rejects* bad clients. A Rust harness test with an intentionally misconfigured client is required.

## Category 14: Auth middleware

`control-plane/src/auth.rs` — `require_api_key` middleware + `Actor` enum + `hash_key`. **Critical gap found**: roles are defined in the DB (`readonly` / `deploy` / `admin`) with a `CHECK` constraint, but **`Actor::has_role` is never called from any route handler** — RBAC is declared but not enforced.

| Item | Code | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|
| `hash_key` (SHA256) | `auth.rs:11-15` | `test_hash_key_deterministic`, `test_hash_key_different_inputs` | Delete (tautological) + Keep logic | Stdlib SHA256 determinism doesn't need a test |
| `Actor` enum + `identifier()` | `auth.rs:19-40` | `test_actor_identifier_apikey`, `test_actor_identifier_machine` | Delete (tautological) | Tests `Display` impl |
| `Actor::has_role` | `auth.rs:34-39` | None | **Fix** + Test | **Never called** — must be wired into admin routes before Phase 3 scenarios |
| `require_api_key` middleware | `auth.rs:44-75` | `test_unauthenticated_request_rejected` (happy + one negative) | Test | Direct test for: missing header, malformed Bearer, unknown token, insufficient role |
| `Actor::Machine` bypass | `auth.rs` | None | Test | Machine actor (from mTLS) must not reach admin routes |
| Bootstrap role = admin | `routes::bootstrap_api_key:442` | None | **Fix** + Test | **Assert**: first key's role is `"admin"`, not configurable |
| Bootstrap idempotency (409 on re-run) | `routes::bootstrap_api_key` | None | **Fix** + Test | Test the `has_api_keys() == false` guard |
| Role × endpoint matrix | multiple handlers | None | **Fix** + Test | Add `actor.has_role(&["deploy"])` calls to deploy-level routes (create_rollout, create_release, etc.); readonly must not be allowed to POST |

**Critical finding:** RBAC is half-built. The enum exists, the DB constraint exists, but no route enforces roles. Phase 2 must either:
- **Option A:** Wire `actor.has_role(...)` into every mutating handler (verdict: **Fix** + Test)
- **Option B:** Collapse roles into a single "admin" level and drop the `role` column entirely (verdict: **Trim**)

**User decision needed at review time** — this is a structural call, not a test gap.

## Category 15: DB migrations

`control-plane/migrations/V1..V10.sql` + `control-plane/src/db.rs::migrate` (refinery-based). V10 is breaking: drops rollouts/batches/events/schedules and recreates them with `release_id` FK.

| Item | File | Current test | Proposed verdict | Rationale |
|---|---|---|---|---|
| `Db::migrate` entry point | `control-plane/src/db.rs:39-45` | `test_migrate_is_idempotent` — **misleading**: only calls `migrate()` once | **Fix** (trivial) | Call `migrate()` twice, assert `Ok` both times |
| V1 initial schema | `V1__initial_schema.sql` | Schema shape assumed | Test | |
| V2 api_keys + role CHECK | `V2__api_keys.sql` | None for CHECK constraint | Test | Insert with invalid role → expect error |
| V3 audit_events + indexes | `V3__audit_events.sql` | Implicit via audit tests | Test | |
| V4 machine_tags composite PK + FK | `V4__machine_tags.sql` | None | Test | FK enforcement (see below) |
| V5 rollouts + rollout_batches | `V5__rollouts.sql` | Later dropped in V10 | Keep — Phase 2 may collapse into V10 | |
| V6 health_reports | `V6__health_reports.sql` | None | Test | |
| V7 rollout_policies | `V7__rollout_policies.sql` | None | **Delete or Test — depends on Cat 1** | |
| V8 rollout_events | `V8__rollout_events.sql` | None | Test | |
| V9 scheduled_rollouts | `V9__scheduled_rollouts.sql` | None | **Delete or Test — depends on Cat 1** | |
| V10 releases (breaking rewrite) | `V10__releases.sql` | None | Test | NOT NULL DEFAULT bug happened here — schema shape test would catch regression |
| Foreign-key enforcement | `db.rs` pragma | **`PRAGMA foreign_keys = ON` not found** — **Fix** | **Fix** | Without this pragma, SQLite does not enforce FK constraints |
| Schema shape validation | No `sqlite_master` query | None | Test | Add post-migrate test: query expected tables exist with expected columns |
| Phase 2 migration rewrite | all | — | — | Spec Section 3 allows rewriting in place; tracked in Phase 2 plan |

**Phase 2 explicit action:** add `conn.execute("PRAGMA foreign_keys = ON", [])?` in `Db::migrate` or connection open. This is a **Fix** — without it, V4's `machine_tags` FK to `machines` is silently unenforced.

## Category 16: Config loading

_To be filled in Task 18._

## Category 17: Agent health check subsystem

_To be filled in Task 19._

## Category 18: Nix interaction layer

_To be filled in Task 20._

## Category 19: Comms layer

_To be filled in Task 21._

## Summary statistics

_To be filled after all categories are populated._

## Archive branch list

_To be filled after verdicts are approved._
