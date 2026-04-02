# Scope System

## Purpose

Scopes are plain NixOS/Darwin/Home Manager modules that self-activate based on `hostSpec` flags. Each scope gates its config with `lib.mkIf hS.<flag>`. Adding a new scope file and importing it in `mkHost` automatically applies it to all hosts with the matching flag.

## Location

- `modules/scopes/_base.nix` -- base scope (plain attrset)
- `modules/scopes/_impermanence.nix` -- impermanence scope (plain attrset)
- `modules/scopes/nixfleet/_agent.nix` -- fleet agent (plain module)
- `modules/scopes/nixfleet/_control-plane.nix` -- control plane (plain module)
- `modules/_shared/host-spec-module.nix` -- flag definitions

## Framework Scopes

The framework ships a small set of scopes. Consuming fleet repos add their own opinionated scopes (graphical, dev, desktop, etc.) on top.

| Scope | File | Flag | Description |
|-------|------|------|-------------|
| [base](base.md) | `scopes/_base.nix` | `!isMinimal` | Universal CLI packages (NixOS + Darwin + HM) |
| [impermanence](impermanence.md) | `scopes/_impermanence.nix` | `isImpermanent` | Btrfs root wipe + system/user persistence paths |
| [nixfleet-agent](nixfleet-agent.md) | `scopes/nixfleet/_agent.nix` | `services.nixfleet-agent.enable` | Fleet management agent systemd service |
| [nixfleet-control-plane](nixfleet-control-plane.md) | `scopes/nixfleet/_control-plane.nix` | `services.nixfleet-control-plane.enable` | Control plane HTTP server |

> **Note:** Opinionated scopes (graphical, dev, desktop, theming, etc.) are not part of the framework. They are defined in consuming fleet repos. The `hostSpec` options for those flags are also declared by the consuming fleet.

## Scope Self-Activation Pattern

Scopes are plain NixOS/Darwin modules that gate with `lib.mkIf`:

```nix
# modules/scopes/_example.nix
{ config, lib, ... }: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.someFlag {
    # ... configuration
  };
}
```

Scopes are plain modules imported directly by `mkHost`.

## Host Flags (replacing roles)

Instead of named roles, hosts set `hostSpec` flags directly (isDev, isGraphical, isServer, isMinimal, etc.). These flags determine which scopes activate. Consuming fleet repos can define shared defaults via a `let` binding in `flake.nix`.

Common flag combinations:

| Pattern | Flags set | Notes |
|---------|-----------|-------|
| Workstation | `isDev`, `isGraphical`, `isImpermanent` | Expects fleet to define dev/graphical/desktop scopes |
| Server | `isServer` | Headless, no graphical or dev tooling |
| Minimal | `isMinimal` | Bare minimum — no base packages |
| macOS workstation | `isDarwin`, `isDev`, `isGraphical` | macOS with dev tools |

Flags are set per-host in the `mkHost` call. Shared defaults can be factored out with a `let` binding.

## Persist Paths Pattern

Impermanence persist paths live alongside their program definitions, not in a central file. Each scope adds its own `home.persistence."/persist".directories` when `isImpermanent` is true.

## Adding a New Scope

1. Create `modules/scopes/_<scope>.nix` as a plain NixOS/Darwin module
2. Gate the config with `lib.mkIf hS.<flag>`
3. Add the flag to `host-spec-module.nix` if new (or extend it in your fleet)
4. Import the scope in `mkHost` — all matching hosts automatically get the scope

## Links

- [Architecture](../architecture.md)
- [Host System](../hosts/README.md)
