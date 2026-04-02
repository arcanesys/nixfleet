# ADR-003: Agent and Control Plane as NixOS Service Modules

**Status:** Accepted
**Date:** 2026-03-31

## Context

NixFleet includes a fleet agent (polls the control plane, runs `nixos-rebuild switch`, reports status) and a control plane (HTTP server managing fleet state). Both need configuration: URLs, TLS certs, auth tokens, poll intervals.

This configuration could live in hostSpec (alongside host identity flags) or in dedicated NixOS service modules (the standard `services.*` pattern).

## Decision

Both are standard NixOS service modules, auto-included by mkHost but disabled by default:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  pollInterval = 60;
};

services.nixfleet-control-plane = {
  enable = true;
  listenAddress = "0.0.0.0";
  port = 8443;
};
```

## Consequences

- Follows NixOS conventions — users expect `services.foo.enable`
- TLS certs and auth tokens wire naturally into secret management (agenix, sops, etc.)
- Per-environment overrides work with standard NixOS semantics (`lib.mkForce`, per-host modules)
- Fleet-wide defaults set in shared fleet modules; per-host overrides in host-specific modules
- hostSpec stays focused on host identity and capability flags, not service configuration
