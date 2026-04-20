# ADR-009: Core Hardening - Subsystem Cuts and Dead Code Removal

**Status:** Accepted
**Date:** 2026-04-10

## Context

A systematic audit of the Rust core (~11K lines, 4 crates) revealed half-built subsystems, tautological tests, and untested critical paths. Several subsystems existed in code but had no users, no tests, and no clear path to completion before launch.

## Decision

### Subsystems deleted

| Subsystem | What was removed |
|---|---|
| **Policy** | CLI `policy *` commands, `/api/v1/policies*` routes, `RolloutPolicy`/`PolicyRequest` types, V7 migration, executor policy resolution |
| **Schedule** | CLI `schedule *` commands, `/api/v1/schedules*` routes, `ScheduledRollout`/`CreateScheduleRequest`/`ScheduleStatus` types, V9 migration, `trigger_due_schedules` |
| **Host provision** | CLI `host provision` subcommand (thin `nixos-anywhere` wrapper - operators use nixos-anywhere directly) |
| **Tag set endpoint** | `POST /machines/{id}/tags` handler and CLI `machines tag` command (redundant with agent auto-sync via health reports) |

All deleted code is preserved on `archive/*` branches.

### Dead code removed

- ~18 tautological tests that tested language features (Clone derive, serde round-trips, SHA256 determinism) rather than behavior
- `MachineHealthStatus::TimedOut` and `MachineHealthStatus::RolledBack` variants (zero external references)
- Unused dependencies: `async-trait` (agent), `tempfile` (agent, control-plane), `serde_json` (shared)

### Fixes applied alongside

- `PRAGMA foreign_keys = ON` enforced on all DB connections
- Bootstrap endpoint writes audit entry (`actor=system:bootstrap`)
- `save_api_key` file permissions set to 0o600
- `test_migrate_is_idempotent` fixed to actually call migrate twice
- mTLS both-or-neither validation enforced
- RBAC `has_role` wired into all admin routes (readonly/deploy/admin tiers)

## Consequences

- ~2700 lines of code removed (net: 201 insertions, 2741 deletions)
- The remaining codebase contains only actively used, tested features
- Removed features can be resurrected from archive branches if needed
- Migration renumbering means existing databases are incompatible (acceptable - no production deployments exist)
