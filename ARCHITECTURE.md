# Architecture

High-level overview of NixFleet. For detailed internals, see [TECHNICAL.md](TECHNICAL.md). For full docs, see [docs/mdbook/](docs/mdbook/).

## Overview

NixFleet is a framework providing `mkHost` -- a single function that returns standard `nixosSystem`/`darwinSystem`. It injects core modules, scopes, and service modules. Fleet repos call `mkHost` and pass their own modules.

## System Overview

```
+-----------------------------------------+
|  Client Fleet (flake.nix)               |
|  mkHost per host, org defaults as let   |
+-----------------------------------------+
|  NixFleet Framework (mkHost)            |
|  Core modules + scopes + services       |
+-----------------------------------------+
|  Rust Workspace                         |
|  Agent <-> Control Plane <-> CLI        |
+-----------------------------------------+
|  NixOS Module System                    |
|  nixosSystem / darwinSystem             |
+-----------------------------------------+
```

## Module Graph

```
mkHost closure (binds framework inputs) ->
  - hostSpec module (base options)
  - disko + impermanence NixOS modules
  - core/_nixos.nix or core/_darwin.nix
  - scopes/_base.nix, scopes/_impermanence.nix
  - scopes/_firewall.nix (auto on !isMinimal)
  - scopes/_secrets.nix, scopes/_backup.nix, scopes/_monitoring.nix (opt-in)
  - services: _agent.nix, _control-plane.nix (disabled by default)
  - home-manager (user config)
  - user-provided modules
```

## Scope Activation

Scopes self-activate via `lib.mkIf config.hostSpec.<flag>`. No registration, no wiring. Adding a scope file and importing it in `mkHost` automatically applies it to all hosts with the matching flag.

## Fleet Integration

Fleet repos import `mkHost`, define org defaults as `let` bindings, call `mkHost` per host. Fleet-specific scopes are plain NixOS/HM modules organized in a module index.

## Framework Inputs

mkHost passes framework inputs (nixpkgs, home-manager, disko, etc.) to modules via `specialArgs = { inherit inputs; }`. Fleet repos access these as the `inputs` argument in their modules. Fleet-specific customization uses hostSpec extensions and plain NixOS modules, not a separate input namespace.

## Data Flow

```
flake.nix (host definitions via mkHost)
    |
    v
nixosConfigurations / darwinConfigurations (Nix outputs)
    |
    v
deploy (nixos-anywhere / nixos-rebuild / darwin-rebuild)
    |
    v
nixfleet-agent (on each host, reports to CP)
    |
    v
nixfleet-control-plane (central registry, orchestration)
    ^
    |
nixfleet CLI (operator commands)
```

## Framework vs Client Separation

**Framework** (`nixfleet` repo): `mkHost`, core modules, scopes, Rust crates, tests. Generic with no org-specific assumptions.

**Client** (your fleet repo): Org defaults as `let` bindings, host definitions via `mkHost`, fleet-specific scopes, secrets wiring, HM programs, wrappers. The framework provides a minimal test fleet for CI.

This separation means an external organization can consume the framework without forking:

```nix
{
  inputs.nixfleet.url = "github:your-org/nixfleet";

  outputs = { nixfleet, ... }:
    let
      mkHost = nixfleet.lib.mkHost;
    in {
      nixosConfigurations.my-host = mkHost { ... };
    };
}
```

## Nix Module Layers

### Core (always active, framework-provided)

`modules/core/` -- boot, networking, user accounts, security, zsh, git. Every host gets these regardless of flags.

### Scopes (flag-gated, framework + fleet-provided)

Scope modules are plain NixOS/Darwin modules that self-activate with `lib.mkIf hS.<flag>` and co-locate impermanence persist paths. Framework scopes: base, impermanence, firewall (automatic); secrets, backup, monitoring, agent, control-plane (opt-in). Fleet repos add their own scopes (dev tools, desktop environments, theming, etc.).

### Fleet-Provided Modules

Home Manager program configs, additional NixOS modules, and fleet-specific scopes live in consuming fleet repos — not in the framework.

## Rust Workspace

Four crates, one Cargo workspace:

| Crate | Type | Purpose |
|-------|------|---------|
| `agent/` | Binary | State machine on each managed host. Registers, polls for config, deploys, reports status |
| `control-plane/` | Binary | Axum HTTP server. Machine registry, deployment scheduling, health tracking |
| `cli/` | Binary | Operator-facing commands: deploy, status, rollback |
| `shared/` | Library | Common types and API contracts shared across crates |

Each Rust binary is packaged as a Nix derivation and can be included in host configurations.

## Repo Structure

| Repo | Content |
|------|---------|
| `nixfleet` (this repo) | Framework lib, core modules, scopes, Rust crates, tests |
| `fleet` (your repo) | Org config, host definitions, fleet scopes, secrets wiring |

Secrets are referenced by path in the public repo. The private repo is a flake input (`inputs.secrets`).

## Key Design Decisions

1. **Dendritic import**: Every `.nix` under `modules/` is auto-imported. No import lists to maintain.
2. **Plain modules**: Scopes are plain NixOS/HM modules imported by `mkHost`. No deferred registration.
3. **Central fleet definition**: All hosts in `flake.nix`, not scattered across directories.
4. **Single API**: `mkHost` is the only public constructor. No mkFleet/mkOrg/mkRole layer.
5. **Scope-aware impermanence**: Persist paths live alongside their program definitions in scopes.
6. **Mechanism over policy**: Framework provides `mkHost`; fleets provide opinions.
