# ADR-002: hostSpec Flags Over Role Hierarchy

**Status:** Accepted
**Date:** 2026-03-31

## Context

Hosts in a fleet differ along multiple axes: graphical vs headless, persistent vs ephemeral, dev vs production, Linux vs macOS. These capabilities could be expressed as named roles (e.g., "workstation" = graphical + dev + persistent) or as independent boolean flags.

Roles reduce repetition but hide what's actually enabled. A "workstation" that includes 4 flags creates a "what does this role include?" question for every new host definition.

## Decision

Each host sets explicit flags in `hostSpec`:

```nix
hostSpec = {
  userName = "admin";
  isImpermanent = true;
  isServer = true;
};
```

The framework defines a minimal set of flags (`isMinimal`, `isDarwin`, `isImpermanent`, `isServer`) plus data fields (`userName`, `hostName`, `timeZone`, `locale`, etc.). Scopes react to these flags via `lib.mkIf` (see [ADR-005](005-scope-self-activation.md)).

Fleet repos extend hostSpec with their own flags via plain NixOS modules (see [ADR-006](006-hostspec-extension.md)).

## Consequences

- Host definitions are fully self-describing — every capability is visible as a flag
- No hidden behavior behind role names
- Repetition across similar hosts mitigated by shared `let` bindings (plain Nix)
- Zero framework concepts beyond "flags control what's enabled"
