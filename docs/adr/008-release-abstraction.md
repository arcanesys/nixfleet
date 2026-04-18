# ADR-008: Release Abstraction for Heterogeneous Fleet Deployment

**Status:** Accepted
**Date:** 2026-04-06

## Context

The rollout API accepted a single `generation_hash` (store path) applied to all target hosts.
This assumed homogeneous deployments -- same closure for every machine. In practice, every NixOS
host produces a unique closure. There was also no automated build-and-cache workflow.

## Decision

Introduce a **release** -- an immutable CP-managed manifest mapping each host to its built store
path. Rollouts now reference a release instead of a single generation hash. The CP resolves
per-host store paths at batch execution time. Agents are unchanged.

Key design choices:
- Releases are CP-managed (not file-based) for audit trail and multi-operator access
- Rollout targeting uses live CP tags, not release entry tags
- Cache-agnostic distribution via two CLI flags:
  - `--push-to <url>` uses `nix copy --to` natively (works with `ssh://`, `s3://`, and any standard Nix binary cache)
  - `--push-hook <cmd>` is an escape hatch for non-standard push protocols (Attic, Cachix). `{}` is replaced with the store path; runs on the `--push-to` host when combined, or locally otherwise
  - `--copy` is a direct SSH `nix-copy-closure` mode that doesn't need a cache at all
- Framework no longer bundles any specific cache implementation -- harmonia is the default cache-server scope (generic Nix binary cache, serves from `/nix/store`), Attic is a fleet-level choice
- No backward compatibility: `generation_hash` removed from rollouts entirely
- Self-build on agents is out of scope: agents are dumb executors

## Consequences

- Fleet deployments are now heterogeneous by default
- Every rollout requires a release (explicit via `--release <ID>` or implicit via `--push-to` / `--copy`)
- The `POST /api/v1/machines/{id}/set-generation` endpoint is removed
- Rollback uses per-machine previous generations stored on each batch
- Releases accumulate in the CP database (retention/GC is future work)
- Attic was moved out of the framework -- consumers who want it add it as a fleet-level flake input and module
