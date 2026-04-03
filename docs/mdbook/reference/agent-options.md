# Agent Options

All options under `services.nixfleet-agent`. The module is auto-included by mkHost and disabled by default.

## Top-level options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet fleet management agent. |
| `controlPlaneUrl` | `str` | -- (required when enabled) | URL of the NixFleet control plane. Example: `"https://fleet.example.com"`. |
| `machineId` | `str` | `config.networking.hostName` | Machine identifier reported to the control plane. |
| `pollInterval` | `int` | `300` | Poll interval in seconds. |
| `cacheUrl` | `nullOr str` | `null` | Binary cache URL for `nix copy --from`. Falls back to control plane default. Example: `"https://cache.fleet.example.com"`. |
| `dbPath` | `str` | `"/var/lib/nixfleet/state.db"` | Path to the SQLite state database. |
| `dryRun` | `bool` | `false` | When true, check and fetch but do not apply generations. |
| `tags` | `listOf str` | `[]` | Tags for grouping this machine in fleet operations. Passed via `NIXFLEET_TAGS` environment variable. |
| `healthInterval` | `int` | `60` | Seconds between continuous health reports to the control plane. |

## healthChecks.systemd

List of systemd unit health checks.

| Sub-option | Type | Default | Description |
|------------|------|---------|-------------|
| `units` | `listOf str` | -- | Systemd units that must be active. |

Example:

```nix
services.nixfleet-agent.healthChecks.systemd = [
  { units = ["nginx.service" "postgresql.service"]; }
];
```

## healthChecks.http

List of HTTP endpoint health checks.

| Sub-option | Type | Default | Description |
|------------|------|---------|-------------|
| `url` | `str` | -- | URL to GET. |
| `interval` | `int` | `5` | Check interval in seconds. |
| `timeout` | `int` | `3` | Timeout in seconds. |
| `expectedStatus` | `int` | `200` | Expected HTTP status code. |

Example:

```nix
services.nixfleet-agent.healthChecks.http = [
  { url = "http://localhost:8080/health"; }
  { url = "https://localhost:443"; expectedStatus = 200; timeout = 5; }
];
```

## healthChecks.command

List of custom command health checks.

| Sub-option | Type | Default | Description |
|------------|------|---------|-------------|
| `name` | `str` | -- | Check name. |
| `command` | `str` | -- | Shell command (exit 0 = healthy). |
| `interval` | `int` | `10` | Check interval in seconds. |
| `timeout` | `int` | `5` | Timeout in seconds. |

Example:

```nix
services.nixfleet-agent.healthChecks.command = [
  {
    name = "disk-space";
    command = "test $(df --output=pcent / | tail -1 | tr -d ' %') -lt 90";
    interval = 30;
    timeout = 5;
  }
];
```

## Systemd service

The agent runs as a systemd service with hardening:

| Setting | Value |
|---------|-------|
| Target | `multi-user.target` |
| After | `network-online.target` |
| Restart | `always` (30s delay) |
| StateDirectory | `nixfleet` |
| NoNewPrivileges | `true` |
| ProtectHome | `true` |
| PrivateTmp | `true` |
| PrivateDevices | `true` |
| ProtectKernelTunables | `true` |
| ProtectKernelModules | `true` |
| ProtectControlGroups | `true` |
| ReadWritePaths | `/var/lib/nixfleet`, `/nix/var/nix` |
| ReadOnlyPaths | `/nix/store`, `/run/current-system` |

Health check configuration is written to `/etc/nixfleet/health-checks.json` and passed via `--health-config`.

On impermanent hosts, `/var/lib/nixfleet` is automatically persisted.
