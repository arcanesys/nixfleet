# Architecture

NixFleet is a fleet management framework providing a declarative API (`mkHost`), Rust service crates for orchestration, and NixOS modules for host configuration. Companion repos provide infrastructure scopes ([nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes)) and compliance controls ([nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance)).

## System Overview

```
Fleet repo (flake.nix)
    |
    | calls mkHost { hostName, platform, hostSpec, modules }
    v
Framework (core + scopes + service modules)
    |
    | produces
    v
nixosSystem / darwinSystem
    |
    | deploy via
    v
nixos-rebuild / nixos-anywhere         (standard)
    or
Agent <-> Control Plane <-> CLI        (orchestrated)
```

`mkHost` is a closure over framework inputs (nixpkgs, home-manager, disko, impermanence, microvm). It returns a `nixosSystem` or `darwinSystem` based on the `platform` argument. For the full module injection order, see [mkHost API reference](reference/mkhost-api.md).

### Module graph

```
mkHost closure (binds framework inputs) ->
  - hostSpec module (identity-only options)
  - disko + impermanence NixOS modules
  - core/_nixos.nix or core/_darwin.nix
  - scopes/nixfleet/_agent.nix (+ _agent_darwin.nix on Darwin)
  - scopes/nixfleet/_control-plane.nix, _cache-server.nix, _cache.nix, _microvm-host.nix
  - user-provided modules (roles, fleet profiles, hardware)
```

### Scope self-activation

Scopes are plain NixOS/HM modules. They are always imported but only activate when their corresponding enable option is set:

```nix
{ config, lib, ... }:
lib.mkIf config.nixfleet.impermanence.enable {
  # persistence paths, btrfs subvolume setup, etc.
}
```

Every host gets every scope module in its module tree, but inactive scopes produce zero config. Roles (from nixfleet-scopes) set the appropriate enable options. Fleet repos follow the same pattern for their own scopes.

### Framework inputs via specialArgs

`mkHost` passes `inputs` (the framework flake's inputs) through `specialArgs`, making them available to all modules as a function argument. Fleet repos that need their own inputs pass them through `_module.args` or additional `specialArgs`.

## Framework Separation

| Repo | Contents |
|------|----------|
| **nixfleet** | `mkHost` API, core modules (nix, SSH, identity), service modules (agent, control plane, cache, microvm), Rust crates, eval/VM tests |
| **nixfleet-scopes** | 17 infrastructure scopes, 4 roles (server, workstation, endpoint, microvm-guest), 6 disk templates |
| **nixfleet-compliance** | 16 compliance controls, 4 regulatory frameworks (NIS2, DORA, ISO 27001, ANSSI), evidence probes |
| **Consumer fleet repos** | Host definitions via `mkHost`, opinionated scopes, hardware configs, secrets wiring |

The framework is generic with no org-specific assumptions. Fleet repos provide opinions. Consumers import scopes via roles or individual scope modules from nixfleet-scopes.

## Rust Workspace

Four crates in a Cargo workspace at the repo root:

| Crate | Binary | Purpose |
|-------|--------|---------|
| `agent/` | `nixfleet-agent` | State machine daemon on each managed host: poll - fetch - apply - verify - report |
| `control-plane/` | `nixfleet-control-plane` | Axum HTTP server with mTLS. Machine registry, rollout orchestration, audit log |
| `cli/` | `nixfleet` | Operator CLI: deploy, status, rollback, release, rollout, machines, bootstrap, init |
| `shared/` | (library) | `nixfleet-types` - shared data types and API contracts |

Agents poll the control plane for a desired generation, fetch closures, apply, run health checks, and report status. The CLI interacts with the control plane for machine registration, lifecycle management, releases, and rollouts.

Both the agent and control plane ship as NixOS service modules, auto-included by `mkHost` but disabled by default. Standard `nixos-rebuild` and `nixos-anywhere` work without them.

- [Agent guide](guide/deploying/agent.md) - [Agent options](reference/agent-options.md)
- [Control Plane guide](guide/deploying/control-plane.md) - [Control Plane options](reference/control-plane-options.md)
- [CLI reference](reference/cli.md)

## Flake Inputs

| Input | Purpose |
|-------|---------|
| `nixpkgs` | Package repository (nixos-unstable) |
| `darwin` | nix-darwin macOS system config |
| `home-manager` | User environment management |
| `flake-parts` | Module system for flake outputs |
| `import-tree` | Auto-import directory tree as modules |
| `disko` | Declarative disk partitioning |
| `impermanence` | Ephemeral root filesystem |
| `nixos-anywhere` | Remote NixOS installation via SSH |
| `nixos-hardware` | Hardware-specific optimizations |
| `lanzaboote` | Secure Boot |
| `treefmt-nix` | Multi-language formatting |
| `microvm` | MicroVM support (microvm.nix) |
| `crane` | Rust build system for Cargo workspace |
| `nixfleet-scopes` | Companion: infrastructure scopes, roles, disk templates |

Fleet repos add their own inputs as needed (e.g. `agenix` or `sops-nix` for secrets).

## Design Decisions

Key architectural decisions are documented in [Architecture Decision Records](adr/).

Summary of foundational decisions:

1. **Dendritic import** - every `.nix` under `modules/` is auto-imported via import-tree. No import lists to maintain.
2. **Plain modules** - scopes are plain NixOS/HM modules imported by `mkHost`. No deferred registration.
3. **Central fleet definition** - all hosts in `flake.nix`, not scattered across directories.
4. **Single API** - `mkHost` is the only public constructor. No mkFleet/mkOrg/mkRole layer.
5. **Scope-aware impermanence** - persist paths live alongside their program definitions in scopes.
6. **Mechanism over policy** - the framework provides `mkHost`; fleets provide opinions.
