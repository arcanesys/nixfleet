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
8. **Service modules** -- `nixfleet/_agent.nix`, `nixfleet/_control-plane.nix`, `nixfleet/_cache-server.nix`, `nixfleet/_cache.nix` (all disabled by default)
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

A systemd daemon on each managed host. Every poll tick runs a single sequential deploy cycle to completion:

```
poll_tick fires
  -> get_desired_generation (CP returns {hash, cache_url, poll_hint})
  -> if current == hash: "up-to-date", done
  -> nix copy --from <cache_url> <hash>    (or nix path-info if no cache)
  -> switch-to-configuration switch
  -> run health checks
  -> POST /report {current_generation, success, message}
  -> on failure: auto-rollback to previous generation, report failure
```

The poll interval adapts to the CP's `poll_hint` — typically `5s` during active rollouts and the configured `pollInterval` (default 300s) otherwise. Failed polls reschedule to `retryInterval` (default 30s) to recover quickly from transient errors and bootstrap races.

A separate health-reporter tick (default 60s) sends periodic health reports to the CP independent of the deploy cycle. Local state is persisted in SQLite (`/var/lib/nixfleet/state.db`); the nix metadata cache lives under `$XDG_CACHE_HOME` inside the state directory.

### Control Plane (`nixfleet-control-plane`)

An Axum HTTP server that acts as the fleet inventory, release registry, and rollout coordinator:

- **Machine registry** -- machine IDs, lifecycle state, tags, current generation (from reports)
- **Releases** -- immutable manifests mapping each host to its built store path. Created by operators, referenced by rollouts multiple times
- **Rollouts** -- create, resume, cancel coordinated multi-machine deployments. Each rollout references a release; the CP resolves per-host store paths from release entries at batch execution time
- **Desired generation** -- set per-machine by the executor during a batch, read by agents via `GET /desired-generation`. The response includes `poll_hint` during active rollouts so agents react within seconds
- **Generation-gated health** -- the executor only accepts a batch's health reports after verifying the machine's `current_generation` matches the desired store path, preventing false-positive completion from stale reports
- **Reports** -- agents POST status reports; the CP auto-syncs tags
- **Audit log** -- records all API mutations
- **Auth** -- mTLS (transport) + API key (Bearer) for admin endpoints

### CLI (`nixfleet-cli`)

Operator tool for interacting with the control plane. Major commands:

- `nixfleet init` -- create a `.nixfleet.toml` config file in the fleet repo
- `nixfleet bootstrap` -- create the first admin API key (auto-saved to `~/.config/nixfleet/credentials.toml`)
- `nixfleet release create/list/show/diff` -- manage releases
- `nixfleet deploy --release <ID>` -- trigger a rollout against a release (or build+push+deploy in one command with `--push-to` / `--copy`)
- `nixfleet status` -- query fleet inventory
- `nixfleet rollout list/status/resume/cancel` -- manage running rollouts
- `nixfleet rollback` -- trigger rollback on a specific machine

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
