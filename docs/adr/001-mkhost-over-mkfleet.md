# ADR-001: Single-Function API

**Status:** Accepted
**Date:** 2026-03-31

## Context

Fleet management frameworks face a design tension: provide a rich DSL with multiple abstractions (fleet → org → role → host) or keep the API surface minimal. More abstractions can reduce boilerplate but increase the learning curve and couple fleet structure to framework opinions.

NixFleet needs an API that lets consumers define hosts without learning framework-specific concepts beyond the minimum necessary.

## Decision

NixFleet exposes a single public function: `nixfleet.lib.mkHost`. It takes a host definition and returns a standard `nixosSystem` or `darwinSystem`.

```nix
nixfleet.lib.mkHost {
  hostName = "web-01";
  platform = "x86_64-linux";
  hostSpec = { userName = "admin"; isImpermanent = true; };
  modules = [ ./hardware/web-01 ./disk-config.nix ];
};
```

Fleet repos define `nixosConfigurations` directly in their flake outputs. Org-level defaults are plain `let` bindings in the fleet's `flake.nix`. There is no fleet, org, or role abstraction.

## Consequences

- Standard NixOS commands work directly: `nixos-anywhere --flake .#host`, `nixos-rebuild switch --flake .#host`
- Zero learning curve beyond "call mkHost, get a nixosConfiguration"
- Fleet repos are standard Nix flakes with no framework-specific flake structure
- Batch hosts are created with standard Nix (`builtins.map` over mkHost)
- Convenience sugar (e.g., auto-discovery from directory structure) can be layered on top without changing the primitive
