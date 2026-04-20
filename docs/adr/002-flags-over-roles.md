# ADR-002: hostSpec as Identity Data, Scopes as Behavior

**Status:** Accepted
**Date:** 2026-03-31 (revised 2026-04-20)

## Context

Hosts in a fleet differ along multiple axes: graphical vs headless, persistent vs ephemeral, dev vs production, Linux vs macOS. Earlier revisions of NixFleet expressed these differences as boolean posture flags inside `hostSpec` (`isImpermanent`, `isServer`, `isMinimal`). Scopes read those flags to decide whether to activate.

This coupled identity data (who is this host?) with behavioral intent (what should this host do?). It also meant the framework had to define every possible posture flag, even though the set of behaviors belongs to the scope layer.

## Decision

**hostSpec carries identity and locale data only.** Behavioral configuration is expressed through scope enable options (`nixfleet.<scope>.enable`). Roles are scope bundles that import scopes and set their enable options with `lib.mkDefault`.

### hostSpec - identity fields only

```nix
# modules/_shared/host-spec-module.nix (simplified)
options.hostSpec = {
  hostName    = lib.mkOption { type = lib.types.str; };
  userName    = lib.mkOption { type = lib.types.str; };
  home        = lib.mkOption { type = lib.types.str; };
  timeZone    = lib.mkOption { type = lib.types.str; default = "UTC"; };
  locale      = lib.mkOption { type = lib.types.str; default = "en_US.UTF-8"; };
  isDarwin    = lib.mkOption { type = lib.types.bool; default = false; };
  secretsPath = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
};
```

### Roles - scope bundles with enable defaults

Roles live in `nixfleet-scopes`. They import scopes and set enable options:

```nix
# nixfleet-scopes/modules/roles/server.nix
{ lib, ... }: {
  imports = [
    ../scopes/base
    ../scopes/operators
    ../scopes/firewall
    ../scopes/secrets
    ../scopes/monitoring
    ../scopes/impermanence
  ];

  config = {
    nixfleet.firewall.enable = lib.mkDefault true;
    nixfleet.secrets.enable = lib.mkDefault true;
    nixfleet.monitoring.nodeExporter.enable = lib.mkDefault true;
  };
}
```

### Scopes - guard on their own enable option

```nix
# nixfleet-scopes/modules/scopes/impermanence/default.nix (simplified)
options.nixfleet.impermanence.enable =
  lib.mkEnableOption "NixFleet system-level impermanence";

config = lib.mkIf cfg.enable {
  environment.persistence.${cfg.persistRoot} = { ... };
};
```

Roles set `lib.mkDefault`, so host definitions can override any scope activation. The framework itself has no concept of "role" - roles are plain NixOS modules in `nixfleet-scopes`.

## Consequences

- hostSpec is a flat data bag with no behavioral flags - it describes identity, not intent
- No inheritance hierarchy, no "role" abstraction in the framework
- Scopes own their activation logic (`lib.mkIf cfg.enable`)
- Roles are just convenience bundles - hosts can bypass them entirely
- Fleet repos extend hostSpec with their own identity fields via plain NixOS modules (see [ADR-006](006-hostspec-extension.md))
