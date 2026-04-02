# nixfleet-agent scope

## Purpose

Plain NixOS service module that runs the NixFleet fleet management agent as a systemd service. The agent polls the control plane, fetches new generations, applies them via `nixos-rebuild`, runs health checks, and reports status. Auto-included by `mkHost`.

## Location

- `modules/scopes/nixfleet/_agent.nix`

## Activation

This is a plain NixOS service module auto-included by `mkHost`. It is disabled by default. Enable it explicitly per host:

```nix
services.nixfleet-agent.enable = true;
```

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Enable the agent service |
| `controlPlaneUrl` | str | -- | URL of the NixFleet control plane (required) |
| `machineId` | str | `hostname` | Machine identifier sent to control plane |
| `pollInterval` | int | 300 | Poll interval in seconds |
| `cacheUrl` | str or null | null | Binary cache URL for pre-fetching closures |
| `dbPath` | str | `/var/lib/nixfleet/state.db` | SQLite state database path |
| `dryRun` | bool | false | Check and fetch but do not apply generations |
| `tags` | list of str | `[]` | Tags for grouping this machine in fleet operations |
| `healthInterval` | int | 60 | Seconds between continuous health reports to control plane |

## Tags

Tags group machines for targeted fleet operations. The control plane uses tags to select which machines participate in a rollout.

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  tags = [ "production" "web" "eu-west" ];
};
```

Tags are passed to the agent via the `NIXFLEET_TAGS` environment variable and sent to the control plane at registration.

## Health Checks

Health checks run continuously on the agent and report results to the control plane. The control plane uses health status to determine whether a rollout batch succeeded or needs rollback.

Three check types are available:

### Systemd checks

Verify that specific systemd units are active:

```nix
services.nixfleet-agent.healthChecks.systemd = [
  { units = [ "nginx.service" "postgresql.service" ]; }
];
```

### HTTP checks

Verify that HTTP endpoints return the expected status:

```nix
services.nixfleet-agent.healthChecks.http = [
  {
    url = "http://localhost:8080/health";
    interval = 5;       # seconds between checks
    timeout = 3;        # seconds before timeout
    expectedStatus = 200;
  }
];
```

### Command checks

Run arbitrary shell commands (exit 0 = healthy):

```nix
services.nixfleet-agent.healthChecks.command = [
  {
    name = "disk-space";
    command = "test $(df --output=pcent / | tail -1 | tr -d ' %') -lt 90";
    interval = 10;
    timeout = 5;
  }
];
```

Health check definitions are serialized to `/etc/nixfleet/health-checks.json` and read by the agent at startup.

## Systemd Hardening

The service runs with NoNewPrivileges, PrivateTmp, PrivateDevices, and restricted read-write paths (`/var/lib/nixfleet`, `/nix/var/nix`). This is a security-sensitive service -- hardening is intentional.

## Impermanence

When `hostSpec.isImpermanent` is true, `/var/lib/nixfleet` is automatically added to `environment.persistence."/persist".directories` so agent state survives reboots.

## Links

- [Scopes Overview](README.md)
- [Control Plane](nixfleet-control-plane.md)
- [CLI Reference](../cli/README.md)
