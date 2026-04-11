# Architecture

```
Fleet repo (flake.nix)
    |
    | calls mkHost { hostName, platform, hostSpec, modules }
    v
Framework (core + scopes + service modules)
    |
    | produces
    v
nixosSystem / darwinSystem
    |
    | deploy via
    v
nixos-rebuild / nixos-anywhere         (standard)
    or
Agent <-> Control Plane <-> CLI        (orchestrated)
```

## mkHost: module composition

`mkHost` is the single framework entry point. It is a closure over framework
inputs (nixpkgs, home-manager, disko, impermanence, microvm) and returns a
`nixosSystem` or `darwinSystem` based on the `platform` argument.

For the full module injection order, see [mkHost API reference](reference/mkhost-api.md).

## Scope self-activation

Scopes are plain NixOS/HM modules. They are always imported but only activate
when their corresponding `hostSpec` flag is set:

```nix
# Simplified pattern from _impermanence.nix
{ config, lib, ... }:
lib.mkIf config.hostSpec.isImpermanent {
  # persistence paths, btrfs subvolume setup, etc.
}
```

This means every host gets every scope module in its module tree, but inactive
scopes produce zero config. Fleet repos follow the same pattern for their own
scopes (graphical, dev, theming, etc.).

For the full scope list and activation conditions, see [Scopes](guide/defining-hosts/scopes.md).

## Framework inputs via specialArgs

`mkHost` passes `inputs` (the framework flake's inputs) through `specialArgs`,
making them available to all modules as a function argument. Fleet repos that
need their own inputs pass them through standard NixOS module mechanisms
(`_module.args` or additional `specialArgs` in their own wrapper).

## Orchestration layer

NixFleet includes three Rust crates for fleet-scale orchestration. They are
optional — standard `nixos-rebuild` and `nixos-anywhere` work without them.

- **Agent** — systemd daemon on each host: poll → fetch → apply → verify → report. [Guide](guide/deploying/agent.md) · [Options](reference/agent-options.md)
- **Control Plane** — Axum HTTP server: machine registry, releases, rollout orchestration, audit log. [Guide](guide/deploying/control-plane.md) · [Options](reference/control-plane-options.md)
- **CLI** — operator tool: deploy, release, rollout, status, machines. [Reference](reference/cli.md)

Both the agent and control plane ship as NixOS service modules, auto-included by `mkHost` but disabled by default.
