# ADR-004: Follows Chain for Dependency Pinning

**Status:** Accepted
**Date:** 2026-03-31

## Context

Fleet repos consume NixFleet as a flake input. Both need nixpkgs, home-manager, disko, and other shared inputs. Two strategies exist: fleet repos pin their own versions independently, or fleet repos inherit NixFleet's pins via `follows`.

Independent pins risk version skew — framework modules tested against one nixpkgs revision may break with a different one (option renames, module changes, package removals).

## Decision

Fleet repos use `follows` to inherit NixFleet's dependency pins:

```nix
{
  inputs.nixfleet.url = "github:your-org/nixfleet";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";
  inputs.home-manager.follows = "nixfleet/home-manager";
  inputs.disko.follows = "nixfleet/disko";
}
```

NixFleet is the source of truth for shared dependency versions. Fleet-specific inputs (themes, editor plugins, etc.) are pinned independently.

## Consequences

- `nix flake update nixfleet` in a fleet repo updates all shared dependencies in one command
- NixFleet must stay reasonably up-to-date with nixpkgs-unstable to avoid blocking consumers
- Fleet repos cannot independently update nixpkgs without breaking the follows chain
- Framework modules are always tested against the exact nixpkgs revision consumers use
