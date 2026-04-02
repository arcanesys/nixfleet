# ADR-005: Scope Self-Activation via hostSpec Flags

**Status:** Accepted
**Date:** 2026-03-31

## Context

NixFleet organizes functionality into "scopes" — modules that provide a cohesive set of features (base packages, impermanence, fleet agent, etc.). These scopes need to activate conditionally based on host configuration.

Two approaches: mkHost could explicitly include/exclude scopes based on flags, or scopes could be always-included and self-activate by guarding their config behind `lib.mkIf`.

## Decision

All scopes are plain NixOS/Home Manager modules, always included by mkHost. Each scope guards its own `config` block with `lib.mkIf` on the relevant hostSpec flag:

```nix
# _impermanence.nix (simplified)
config = lib.mkIf hS.isImpermanent {
  environment.persistence."/persist/system" = {
    directories = [ "/etc/nixos" "/var/lib/systemd" "/var/log" ];
  };
};
```

```nix
# _base.nix (simplified)
environment.systemPackages = with pkgs;
  lib.optionals (!hS.isMinimal) [ unixtools.ifconfig xdg-utils ];
```

Service modules (agent, control plane) follow the same pattern using `lib.mkIf cfg.enable`.

## Consequences

- mkHost has no conditional logic — it includes everything, scopes decide for themselves
- Adding a new scope requires no changes to mkHost — just create the module and import it
- Scope activation is visible in the scope's own code, not hidden in mkHost wiring
- Persist paths live alongside their program definitions (scope-aware impermanence), not centralized
- NixOS lazy evaluation means inactive scopes add zero overhead
