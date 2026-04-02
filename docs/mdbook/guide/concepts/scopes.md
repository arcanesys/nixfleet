# The Scope System

How features are organized and self-activate.

## What Are Scopes?

Scopes are feature groups that activate based on host flags. Instead of manually listing packages and services for each host, you set a flag and the scope handles everything. Scopes are plain NixOS/Darwin/HM modules imported by `mkHost`.

### Framework Scopes

| Flag | Scope | What it provides |
|------|-------|-----------------|
| `!isMinimal` | base | Universal CLI packages |
| `isImpermanent` | impermanence | Ephemeral root, persist paths, btrfs wipe |

### Fleet-Defined Scopes (examples)

Consuming fleets add their own scopes for opinionated features:

| Flag | Scope | What it might provide |
|------|-------|----------------------|
| `isDev` | dev | direnv, docker, build tools |
| `isGraphical` | graphical | audio, fonts, browsers |
| `useMyCompositor` | desktop | Wayland compositor, display manager |

## Self-Activation

Each scope module checks its flag and activates itself:

```nix
# Example fleet scope
config = lib.mkIf hS.isDev {
  virtualisation.docker.enable = true;
  environment.systemPackages = [ ... ];
};
```

When you add `isDev = true` to a host, it gets everything in the dev scope -- without listing any of it explicitly.

## Plain Module Pattern

Scopes are plain NixOS/Darwin modules imported directly by `mkHost`. They self-gate with `lib.mkIf` -- just a module with a conditional.

## Adding a Feature

To add a new feature to an existing scope:
1. Edit the scope's module file
2. The feature appears on every host with that flag
3. No host files need changing

To add a new scope, see [Adding a New Scope](../advanced/new-scope.md).

## Scope-Aware Persistence

Each scope manages its own persist paths. When a scope adds a program that needs persistent state, the persist path lives in the same scope module -- not in a central file.

## Host Flags

Instead of named roles, hosts set `hostSpec` flags directly. These flags determine which scopes activate. Shared defaults can be factored out via a `let` binding in `flake.nix`.

Common flag combinations:

| Pattern | Flags set |
|---------|-----------|
| Workstation | `isDev`, `isGraphical`, `isImpermanent` |
| Server | `isServer` |
| Minimal | `isMinimal` |
| macOS workstation | `isDarwin`, `isDev` |

## Platform Awareness

Scopes handle platform differences internally. A dev scope can install Docker on NixOS and skip it on macOS. Graphical scopes only apply to NixOS (macOS handles graphics differently).

## Further Reading

- [Technical Scope Details](../../scopes/README.md) -- framework scope modules
- [Adding a New Scope](../advanced/new-scope.md) -- step-by-step guide
