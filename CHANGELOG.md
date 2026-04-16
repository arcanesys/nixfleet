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
- `hostSpec.managedUser` (default `true`) — opt out of framework user creation. Useful for hosts where another module owns the user set (e.g. Sécurix endpoints with an operator inventory).
- `hostSpec.enableHomeManager` (default `true`) — opt out of Home Manager injection. Useful for locked-down endpoints that don't use per-user HM config.
- `hostSpec.customFilesystems` (default `false`) — skip built-in qemu disk imports when `isVm = true`. Useful for hosts that provide their own disko layout.
- `hostSpec.skipDefaultFirewall` (default `false`) — skip the firewall scope activation. Useful for hosts whose consuming modules own the firewall (e.g. strict VPN firewall).
- Test host `endpoint-01` in the framework test fleet exercising all four flags, plus four `eval-endpoint-*` eval checks asserting the flags take effect.

### Changed

- `boot.loader.systemd-boot.enable`, `boot.loader.systemd-boot.configurationLimit`, and `boot.loader.efi.canTouchEfiVariables` in `core/_nixos.nix` are now `lib.mkDefault`. Consumers using lanzaboote (Secure Boot) or alternative boot loaders can override without `lib.mkForce`.

[0.1.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0
