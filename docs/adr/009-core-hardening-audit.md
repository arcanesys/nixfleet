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

_To be filled in Task 5._

## Category 4: Executor features

_To be filled in Task 6._

## Category 5: Shared types

_To be filled in Task 7._

## Category 6: Dependencies

_To be filled in Task 8._

## Category 7: Tautological tests

_To be filled in Task 9._

## Category 8: `#[allow(dead_code)]` escapes

_To be filled in Task 10._

## Category 9: Background tasks

_To be filled in Task 11._

## Category 10: State hydration

_To be filled in Task 12._

## Category 11: Prometheus metrics emission

_To be filled in Task 13._

## Category 12: Audit logging

_To be filled in Task 14._

## Category 13: TLS / mTLS

_To be filled in Task 15._

## Category 14: Auth middleware

_To be filled in Task 16._

## Category 15: DB migrations

_To be filled in Task 17._

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
