# ADR-008: Release Abstraction for Heterogeneous Fleet Deployment

## Status

Accepted

## Context

The rollout API accepted a single `generation_hash` (store path) applied to all target hosts.
This assumed homogeneous deployments — same closure for every machine. In practice, every NixOS
host produces a unique closure. There was also no automated build-and-cache workflow.

## Decision

Introduce a **release** — an immutable CP-managed manifest mapping each host to its built store
path. Rollouts now reference a release instead of a single generation hash. The CP resolves
per-host store paths at batch execution time. Agents are unchanged.

Key design choices:
- Releases are CP-managed (not file-based) for audit trail and multi-operator access
- Rollout targeting uses live CP tags, not release entry tags
- Two distribution modes: `--push` (Attic cache) and `--copy` (SSH nix-copy-closure)
- No backward compatibility: generation_hash removed from rollouts entirely
- Self-build on agents is out of scope: agents are dumb executors

## Consequences

- Fleet deployments are now heterogeneous by default
- Every deployment requires a release (explicit or implicit via --push/--copy)
- The `POST /api/v1/machines/{id}/set-generation` endpoint is removed
- Rollback uses per-machine previous generations stored on each batch
- Releases accumulate in the CP database (retention/GC is future work)
