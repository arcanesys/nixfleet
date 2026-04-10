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

_To be filled in Task 3._

## Category 2: Control-plane HTTP endpoints

_To be filled in Task 4._

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
