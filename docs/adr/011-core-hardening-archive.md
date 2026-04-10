# ADR 011 — Core Hardening Archive Branches

**Date:** 2026-04-10
**Status:** Accepted
**Cycle:** Core hardening (spec: `docs/superpowers/specs/2026-04-10-core-hardening-cycle-design.md`, Phase 2)

## Context

Phase 2 of the core hardening cycle (PR #2) removes three user-visible subsystems and several dead code paths from the nixfleet Rust core. To preserve the removed code for reference or resurrection, this ADR lists each archive branch alongside the SHA it points at.

All archive branches point at the same SHA: the last commit on `main` before the Phase 2 cut.

## Archive SHA

`2616215393a8dedeebe38e8f04e70e941aa07ffa`

## Branches

| Branch | Removed | Resurrect with |
|---|---|---|
| `archive/policy-subsystem` | CLI `policy *`, `/api/v1/policies*` routes, `RolloutPolicy`/`PolicyRequest` types, V7 migration, executor policy resolution | `git checkout archive/policy-subsystem -- cli/src/policy.rs control-plane/src/rollout/policy.rs control-plane/migrations/V7__rollout_policies.sql` |
| `archive/schedule-subsystem` | CLI `schedule *`, `/api/v1/schedules*` routes, `ScheduledRollout`/`CreateScheduleRequest`/`ScheduleStatus` types, V9 migration, executor `trigger_due_schedules` | `git checkout archive/schedule-subsystem -- cli/src/schedule.rs control-plane/src/rollout/schedule.rs control-plane/migrations/V9__scheduled_rollouts.sql` |
| `archive/host-provision` | CLI `host provision` subcommand and its `provision_host` helper | `git checkout archive/host-provision -- cli/src/host.rs` |
| `archive/tag-set-endpoint` | `POST /api/v1/machines/{id}/tags` handler (manual override) and CLI `machines tag` subcommand | `git checkout archive/tag-set-endpoint -- control-plane/src/routes.rs cli/src/machines.rs` |
| `archive/tautological-tests` | ~18 deleted test functions across `agent/`, `control-plane/`, `shared/` | `git log archive/tautological-tests` to locate individual tests |
| `archive/unused-deps` | `Cargo.toml` state before `cargo machete` cleanup | `git checkout archive/unused-deps -- agent/Cargo.toml control-plane/Cargo.toml shared/Cargo.toml` |
| `archive/dead-machine-health-variants` | `MachineHealthStatus::TimedOut`, `MachineHealthStatus::RolledBack` variants | `git checkout archive/dead-machine-health-variants -- shared/src/rollout.rs` |

## Consequences

- Removed code is recoverable but intentionally excluded from `main` history.
- Refinery checksum mismatches from migration renumbering are expected and acceptable; there are no existing deployments to preserve.
- Deletions tracked in PR #2; see `docs/adr/009-core-hardening-audit.md` for the decision table that drove them.
