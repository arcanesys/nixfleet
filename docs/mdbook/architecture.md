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
inputs (nixpkgs, home-manager, disko, impermanence) and returns a
`nixosSystem` or `darwinSystem` based on the `platform` argument.

Modules are injected in this order:

1. **Platform pin** -- `nixpkgs.hostPlatform = platform`
2. **hostSpec module** -- declares the option namespace (`hostSpec.*`)
3. **hostSpec values** -- fleet-supplied values wrapped in `lib.mkDefault` (overridable)
4. **hostName override** -- `hostSpec.hostName` set without `mkDefault` (must match)
5. **Input modules** -- `disko`, `impermanence` (NixOS only)
6. **Core module** -- `core/_nixos.nix` or `core/_darwin.nix` (SSH hardening, firewall, base packages, user creation)
7. **Scopes** -- `_base` (universal CLI tools), `_impermanence` (btrfs root wipe + persistence)
8. **Service modules** -- `nixfleet/_agent.nix`, `nixfleet/_control-plane.nix` (disabled by default)
9. **VM hardware** -- QEMU disk + hardware config (only when `isVm = true`)
10. **Home Manager** -- wired for the primary user with HM versions of the above scopes
11. **Fleet modules** -- the caller's `modules` list (hardware config, disk layout, custom modules)

For Darwin hosts, steps 5 and 8-9 are skipped and `darwinSystem` is called instead.

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

## Framework inputs via specialArgs

`mkHost` passes `inputs` (the framework flake's inputs) through `specialArgs`,
making them available to all modules as a function argument. Fleet repos that
need their own inputs pass them through standard NixOS module mechanisms
(`_module.args` or additional `specialArgs` in their own wrapper).

## Orchestration layer

NixFleet includes three Rust crates for fleet-scale orchestration. They are
optional -- standard `nixos-rebuild` and `nixos-anywhere` work without them.

### Agent (`nixfleet-agent`)

A systemd daemon on each managed host. Runs a state machine:

```
Idle -> Checking -> Fetching -> Applying -> Verifying -> Reporting -> Idle
                                    |            |
                                    v            v
                                RollingBack -> Reporting -> Idle
```

- **Idle**: waits for the poll interval (default 300s)
- **Checking**: queries the control plane for the desired generation, compares with `/run/current-system`
- **Fetching**: downloads the closure from the binary cache (`nix copy --from`)
- **Applying**: runs `switch-to-configuration switch`
- **Verifying**: executes configured health checks (systemd unit status, HTTP probes)
- **RollingBack**: reverts to the previous generation on failure
- **Reporting**: sends success/failure status to the control plane

State is persisted in a local SQLite database (`/var/lib/nixfleet/state.db`).

### Control Plane (`nixfleet-control-plane`)

An Axum HTTP server that acts as the fleet inventory and coordination hub:

- **Machine registry** -- tracks machine IDs, system state, current/desired generations, tags
- **Desired generation** -- operators set a target store path per machine; agents poll for it
- **Rollouts** -- create, resume, cancel coordinated multi-machine deployments
- **Reports** -- agents POST status reports after each deploy cycle
- **Audit log** -- records all API mutations with timestamps
- **Auth** -- API key authentication middleware

### CLI (`nixfleet-cli`)

Operator tool for interacting with the control plane:

- `nixfleet deploy` -- set desired generation for one or more machines
- `nixfleet status` -- query fleet inventory and machine state
- `nixfleet rollout` -- create and manage coordinated rollouts
- `nixfleet rollback` -- trigger rollback on specific machines

### NixOS integration

Both the agent and control plane ship as NixOS service modules:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://cp.example.com:8080";
  tags = [ "web" "production" ];
  healthChecks.http = [{ url = "http://localhost:80/health"; }];
};

services.nixfleet-control-plane = {
  enable = true;
  openFirewall = true;
};
```

These modules are auto-included by `mkHost` but disabled by default. Enable them
in your fleet's host modules.

---

For detailed usage, see the guide sections:
[Deploying > Control Plane](guide/deploying/control-plane.md),
[Deploying > Agent](guide/deploying/agent.md),
[Deploying > Rollouts](guide/deploying/rollouts.md),
[mkHost API reference](reference/mkhost-api.md).
