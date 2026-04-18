# ADR-011: Archive Branches for Removed Subsystems

**Status:** Accepted
**Date:** 2026-04-10
**Related:** [ADR-009](009-core-hardening-audit.md)

## Context

The core hardening (ADR-009) removed several subsystems and dead code paths. To preserve the removed code for reference or resurrection, each category was archived to a named branch before deletion.

## Archive Branches

All branches point at SHA `2616215393a8dedeebe38e8f04e70e941aa07ffa` (last commit on `main` before the hardening cut).

| Branch | Contains | Resurrect with |
|---|---|---|
| `archive/policy-subsystem` | CLI `policy *`, CP routes, types, V7 migration, executor resolution | `git checkout archive/policy-subsystem -- cli/src/policy.rs control-plane/src/rollout/policy.rs control-plane/migrations/V7__rollout_policies.sql` |
| `archive/schedule-subsystem` | CLI `schedule *`, CP routes, types, V9 migration, `trigger_due_schedules` | `git checkout archive/schedule-subsystem -- cli/src/schedule.rs control-plane/src/rollout/schedule.rs control-plane/migrations/V9__scheduled_rollouts.sql` |
| `archive/host-provision` | CLI `host provision` subcommand | `git checkout archive/host-provision -- cli/src/host.rs` |
| `archive/tag-set-endpoint` | `POST /machines/{id}/tags` handler, CLI `machines tag` | `git checkout archive/tag-set-endpoint -- control-plane/src/routes.rs cli/src/machines.rs` |
| `archive/tautological-tests` | ~18 deleted test functions | `git log archive/tautological-tests` to locate individual tests |
| `archive/unused-deps` | Pre-cleanup `Cargo.toml` state | `git checkout archive/unused-deps -- agent/Cargo.toml control-plane/Cargo.toml shared/Cargo.toml` |
| `archive/dead-machine-health-variants` | `MachineHealthStatus::TimedOut`, `RolledBack` | `git checkout archive/dead-machine-health-variants -- shared/src/rollout.rs` |

## Consequences

- Removed code is recoverable from these branches
- Migration renumbering means resurrected migrations need manual re-sequencing
- Archive branches are read-only references; new development should start fresh
