# Mixed-Fleet Deployment (NixOS + Darwin)

NixFleet manages mixed fleets of NixOS servers and macOS workstations. Darwin hosts
run the same agent (via launchd instead of systemd) and participate in the full
fleet lifecycle: health checks, deployments, rollbacks.

## Cross-Platform Builds

An operator on macOS cannot `nix build` Linux closures natively (and vice versa).
Two approaches solve this:

### Option 1: Remote Builders (transparent)

Configure remote builders in `~/.config/nix/nix.conf` or `/etc/nix/nix.conf`:

    builders = ssh://root@linux-builder x86_64-linux
    builders-use-substitutes = true

On macOS with nix-darwin, the simplest setup is the built-in Linux builder VM:

    # In your darwin configuration:
    nix.linux-builder.enable = true;

With remote builders configured, `nix build` delegates transparently — all
`nixfleet` commands work unchanged.

### Option 2: NixFleet Builder Config

Configure builders per platform in `.nixfleet.toml`:

    [builders]
    aarch64-darwin = "ssh://user@mac-builder"
    aarch64-linux = "ssh://root@arm-builder"
    x86_64-linux = "ssh://root@linux-builder"

Set up via `nixfleet init`:

    nixfleet init --control-plane-url https://cp:8080 \
      --builder aarch64-darwin=ssh://user@mac \
      --builder x86_64-linux=ssh://root@linux-box

Override per-command:

    nixfleet release create . --builder aarch64-darwin=ssh://user@other-mac

When a target platform differs from the operator's platform, NixFleet
automatically passes `--builders` to `nix build`. If no builder is
configured, the command fails with an actionable error message.

### Option 3: CI + Eval-Only Releases (recommended for production)

Build all platforms in CI, push closures to a binary cache, then create
releases without building locally:

    # CI builds and pushes all closures to cache
    nix build .#nixosConfigurations.web-01.config.system.build.toplevel
    nix build .#darwinConfigurations.aether.config.system.build.toplevel
    nix copy --to s3://fleet-cache /nix/store/...

    # Operator creates release (no build, just evaluates store paths)
    nixfleet release create . --eval-only --cache-url https://cache.fleet.example.com

`--eval-only` evaluates `config.system.build.toplevel.outPath` instantly on any
platform. It assumes closures exist in the configured cache.

## Compliance

NIS2 compliance evidence collection runs on NixOS hosts only. Darwin workstations
participate in fleet orchestration (agent, health checks, deployments) but are not
subject to compliance probes — NIS2 Article 21 controls target infrastructure, not
developer workstations.

## Darwin Agent

The Darwin agent uses launchd instead of systemd:

- Service: `launchd.daemons.nixfleet-agent` (Label: `com.nixfleet.agent`)
- Auto-restart: `KeepAlive = true` (equivalent to systemd `Restart=always`)
- Logs: `/var/log/nixfleet-agent.log`
- State: `/var/lib/nixfleet/state.db`
- Health checks: `launchd` type (checks service labels via `launchctl`) instead of `systemd` type

Health check config uses `launchd` instead of `systemd`:

    services.nixfleet-agent.healthChecks = {
      launchd = [{ labels = ["com.example.myservice"]; }];
      http = [{ url = "http://localhost:8080/health"; }];
    };

## Activation

Darwin hosts use `<store_path>/activate` + profile update instead of
`switch-to-configuration switch`. The agent handles this automatically —
no operator action needed beyond enabling the agent in the darwin configuration.
