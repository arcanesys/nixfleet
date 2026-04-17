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
  - hostSpec module (identity-only options)
  - disko + impermanence NixOS modules
  - core/_nixos.nix or core/_darwin.nix
  - scopes/nixfleet/_agent.nix (+ _agent_darwin.nix on Darwin)
  - scopes/nixfleet/_control-plane.nix, _cache-server.nix, _cache.nix, _microvm-host.nix
  - user-provided modules (including nixfleet-scopes roles, fleet profiles, hardware)
```

Infrastructure scopes (base, firewall, secrets, backup, monitoring, home-manager, disko) and roles live in [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). Consumers import them via roles or individual scope modules.

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

**Client** (your fleet repo): Org defaults as `let` bindings, host definitions via `mkHost`, fleet-specific scopes, secrets wiring. See `examples/` for consumption patterns.

## Rust Workspace

See [TECHNICAL.md](TECHNICAL.md) for crate details, agent/CP communication, lifecycle states, and flake inputs.

## Key Design Decisions

1. **Dendritic import**: Every `.nix` under `modules/` is auto-imported. No import lists to maintain.
2. **Plain modules**: Scopes are plain NixOS/HM modules imported by `mkHost`. No deferred registration.
3. **Central fleet definition**: All hosts in `flake.nix`, not scattered across directories.
4. **Single API**: `mkHost` is the only public constructor. No mkFleet/mkOrg/mkRole layer.
5. **Scope-aware impermanence**: Persist paths live alongside their program definitions in scopes.
6. **Mechanism over policy**: Framework provides `mkHost`; fleets provide opinions.
