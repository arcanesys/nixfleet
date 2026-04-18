# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.2.0] - Unreleased

### Changed — Scopes extraction (Phase 2 of the extraction plan)

Framework slimmed to mechanism-only. Opinions (base CLI tools, firewall,
secrets, backup, monitoring, impermanence, home-manager, disko) moved
to the new `arcanesys/nixfleet-scopes` repository. Consumers compose
them via the `modules` argument of `mkHost`, typically through a role
import (`inputs.nixfleet-scopes.scopes.roles.{server,workstation,endpoint,microvm-guest}`).

- **`modules/scopes/_{base,firewall,secrets,backup,monitoring,impermanence}.nix`** — moved to nixfleet-scopes
- **`modules/_shared/disk-templates/{btrfs,btrfs-impermanence}-disk.nix`** — moved to nixfleet-scopes
- **`modules/_shared/lib/mk-host.nix`** — slimmed from 178 to 111 lines. Removed:
  - Home Manager injection (HM is now a scope; consumers opt in via roles)
  - Disko auto-import (moved to the nixfleet-scopes `disko` scope, or consumed directly)
  - Auto-import of the six moved scopes
  - Still auto-imports `inputs.nixfleet-scopes.scopes.impermanence` because
    nixfleet's own internal service modules (agent, control-plane,
    microvm-host) conditionally contribute to `environment.persistence.*`.
    The scope is inert when `nixfleet.impermanence.enable = false` (default).
- **`modules/_shared/host-spec-module.nix`** — identity-only. Removed `isImpermanent`, `isServer`, `isMinimal` posture flags — their roles are played by per-scope `nixfleet.<scope>.enable` options (set by roles in nixfleet-scopes).
- **`modules/core/_nixos.nix`** — stripped to universal mechanism:
  - Kept: nix settings, openssh hardening, identity pass-through from hostSpec, root authorized_keys, minimal package set (git, inetutils)
  - Removed: primary user creation (now lives in workstation/server roles), bootloader defaults (now in host-specific hardware-configuration.nix or disk templates), `programs.{zsh,git,gnupg,dconf}`, `security.sudo` rules, `hardware.ledger`
- **`modules/core/_darwin.nix`** — stripped to universal mechanism:
  - Kept: nix settings, TouchID PAM, `local.dock` option declaration, `system.primaryUser` pass-through
  - Removed: primary user creation, `system.defaults.*` trackpad/finder/dock behavior opinions
- **`modules/_hardware/qemu/hardware-configuration.nix`** — now owns bootloader defaults for test VMs (`boot.loader.systemd-boot.enable`, `efi.canTouchEfiVariables`)
- **`modules/_hardware/qemu/disk-config.nix`** — imports disko NixOS module directly and uses the btrfs-impermanence disk template from nixfleet-scopes
- **`modules/fleet.nix`** — test hosts now compose via roles:
  - web-01, web-02, srv-01, secrets-test, cache-test, microvm-test → `scopes.roles.server`
  - dev-01, agent-test, infra-test, backup-restic-test → `scopes.roles.workstation`
  - edge-01 → bare mkHost (no role)
- **`flake.nix`** — added `nixfleet-scopes` input (follows nixpkgs)
- **`diskoTemplates`** flake output — now re-exports `inputs.nixfleet-scopes.scopes.disk-templates` for back-compat

### Deferred

- **VM tests** (Tier A — `pkgs.testers.nixosTest`-based) are gated out of `flake.checks` because the classic `testers.nixosTest` API does not accept `specialArgs`, and nixfleet-scopes roles need `inputs` at module-import time (for `inputs.home-manager.nixosModules.home-manager`). VM test code is intact; migration to `testers.runNixOSTest` is tracked in TODO.md.

### Added — Phase 1 groundwork (in `arcanesys/nixfleet-scopes`)

- 8 generic infrastructure scopes with per-scope `nixfleet.<scope>.*` option namespaces
- 4 generic roles (`server`, `workstation`, `endpoint`, `microvm-guest`)
- 2 platform shims, 2 disk templates

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
[0.2.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.2.0
