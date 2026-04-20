# Mixed-Fleet Deployment (NixOS + Darwin)

NixFleet manages mixed fleets of NixOS servers and macOS workstations. Darwin hosts
run the same agent (via launchd instead of systemd) and participate in the full
fleet lifecycle: health checks, deployments, rollbacks.

## Cross-Platform Builds

An operator on one platform cannot `nix build` closures for another platform
without a remote builder. This applies to all combinations: Linux ↔ Darwin,
x86_64 ↔ aarch64.

### Remote Builders (nix.buildMachines)

Configure remote builders in your NixOS/nix-darwin configuration:

    # On a Linux machine that needs to build Darwin closures:
    nix.buildMachines = [{
      hostName = "aether";
      systems = ["aarch64-darwin"];
      sshUser = "s33d";
      sshKey = "/root/.ssh/id_ed25519";
      maxJobs = 4;
    }];
    nix.distributedBuilds = true;

    # On a Darwin machine that needs to build Linux closures:
    nix.buildMachines = [{
      hostName = "krach";
      systems = ["x86_64-linux"];
      sshUser = "root";
      sshKey = "/root/.ssh/id_ed25519";
      maxJobs = 4;
    }];
    nix.distributedBuilds = true;

On macOS with nix-darwin, the simplest Linux builder is the built-in VM:

    nix.linux-builder.enable = true;

With remote builders configured, `nix build` delegates transparently - all
`nixfleet` commands work unchanged.

**Requirements:**
- Root on the operator's machine needs SSH access to the builder (key-based, no password)
- The builder's host key must be in root's `known_hosts`
- The builder needs nix installed with the target platform's store

### CI + Eval-Only Releases (recommended for production)

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
subject to compliance probes - NIS2 Article 21 controls target infrastructure, not
developer workstations.

## TLS and Custom CA

Fleets using a private CA (e.g. `fleet-ca.pem`) need the agent to trust it.
On NixOS, `security.pki.certificateFiles` adds the CA to the system trust
store. On Darwin, this option doesn't exist.

The agent accepts `--ca-cert` (or `tls.caCert` in the Nix module) to add a
CA certificate alongside system roots. This works on both platforms:

    services.nixfleet-agent.tls.caCert = "/etc/nixfleet/fleet-ca.pem";

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
`switch-to-configuration switch`. The agent handles this automatically -
no operator action needed beyond enabling the agent in the darwin configuration.

## SSH Deploy vs CP Rollout

For Darwin hosts, the **CP rollout path is recommended** - the agent runs
as root via launchd, pulls closures from the binary cache, and activates
locally. No SSH user configuration or sudo setup needed.

The `--ssh` deploy path works for Darwin but has additional requirements
compared to NixOS:

- Connects as `$USER@host` (macOS disables root SSH login)
- Requires the operator's username to exist on the target with SSH key access
- Requires passwordless sudo for `nix-env` and `activate` on the target
- Use `--target user@host` to override the username for single-host deploys

See the [CLI reference](../../reference/cli.md#deploy) for the full Darwin SSH
deploy requirements.
