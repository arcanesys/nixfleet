# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

### Added

- **Framework:** `mkHost` API with `hostSpec` attribute sets, scope-based module composition, disko disk templates
- **Scopes:** base, impermanence, firewall, secrets, backup, monitoring, agent, control-plane, cache-server, cache-client, microvm-host
- **Agent:** Rust daemon with state-machine lifecycle, mTLS authentication, health checks, fire-and-forget apply with self-switch resilience
- **Control plane:** Machine registry, rollout orchestration with policies and schedules, generation tracking, admin API with mTLS and API key auth
- **CLI:** `deploy`, `status`, `rollback`, `oplog` commands for fleet management
- **Testing:** 12 VM fleet scenario tests (bootstrap, release, revert, mTLS, timeout, and more), integration tests, eval tests, `nix run .#validate` with `--rust`/`--vm`/`--all` tiered flags
- **Documentation:** mdBook guide and reference, 12 ADRs, ARCHITECTURE.md, TECHNICAL.md
- **CI:** nix fmt and eval checks on PR, mdBook auto-deploy

[0.1.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0
