# ADR-006: hostSpec Extension by Fleet Repos

**Status:** Accepted
**Date:** 2026-03-31

## Context

The framework defines a minimal set of hostSpec options (identity fields + capability flags). Fleet repos need additional flags for their opinionated scopes (e.g., `isGraphical`, `isDev`, `useHyprland`, `theme`).

Two approaches: the framework could define all possible flags (bloating the framework with fleet-specific concerns), or fleet repos could extend hostSpec with their own options.

## Decision

Fleet repos extend hostSpec by declaring additional options in plain NixOS modules, passed to mkHost via the `modules` parameter:

```nix
# fleet module: modules/hostspec-extensions.nix
{ lib, ... }: {
  options.hostSpec = {
    isGraphical = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable graphical desktop environment";
    };
    isDev = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable development tools";
    };
  };
}
```

```nix
# fleet flake.nix
nixfleet.lib.mkHost {
  hostName = "workstation";
  platform = "x86_64-linux";
  hostSpec = { isGraphical = true; isDev = true; };
  modules = [ ./modules/hostspec-extensions.nix ./hardware/workstation ];
};
```

The NixOS module system merges option declarations from multiple modules, so fleet extensions compose naturally with framework-defined options.

## Consequences

- Framework stays minimal — no fleet-specific flags in the framework codebase
- Fleet repos have full control over their configuration model
- Multiple fleet repos can define different extensions without conflicting
- Type safety preserved — NixOS module system validates flag types and catches typos
- Fleet scopes use extended flags the same way framework scopes use built-in flags (`lib.mkIf hS.isGraphical`)
