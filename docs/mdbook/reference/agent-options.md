# Agent Options

All options under `services.nixfleet-agent`. The module is auto-included by mkHost and disabled by default.

## Top-level options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet fleet management agent. |
| `controlPlaneUrl` | `str` | -- (required when enabled) | URL of the NixFleet control plane. Example: `"https://fleet.example.com"`. |
| `machineId` | `str` | `config.networking.hostName` | Machine identifier reported to the control plane. |
| `pollInterval` | `int` | `60` | Steady-state poll interval in seconds. The control plane may override this for individual cycles via a `poll_hint` field in the desired-generation response (set to `5` during active rollouts), letting the agent react to new deploys within seconds without reducing the steady-state polling rate. |
| `retryInterval` | `int` | `30` | Retry interval in seconds after a failed poll (network error, CP not ready, fetch failure, bootstrap race). Shorter than `pollInterval` so the agent recovers quickly from transient failures without flooding the CP. |
| `cacheUrl` | `nullOr str` | `null` | Global binary cache URL for fetching closures. Resolution order: (1) per-generation `cache_url` from the release entry; (2) this option if set; (3) if neither is set, the agent verifies the store path exists locally via `nix path-info` - the path must be pre-pushed out-of-band. Example: `"http://cache:5000"`. |
| `dbPath` | `str` | `"/var/lib/nixfleet/state.db"` | Path to the SQLite state database. |
| `dryRun` | `bool` | `false` | When true, check and fetch but do not apply generations. |
| `tags` | `listOf str` | `[]` | Tags for grouping this machine in fleet operations. Passed via `NIXFLEET_TAGS` environment variable. |
| `healthInterval` | `int` | `60` | Seconds between continuous health reports to the control plane. |
| `allowInsecure` | `bool` | `false` | Allow insecure HTTP connections to the control plane. Development only. |
| `tls.clientCert` | `nullOr str` | `null` | Path to client certificate PEM file for mTLS authentication. Example: `"/run/secrets/agent-cert.pem"`. |
| `tls.clientKey` | `nullOr str` | `null` | Path to client private key PEM file for mTLS authentication. Example: `"/run/secrets/agent-key.pem"`. |
| `metricsPort` | `nullOr port` | `null` | Port for agent Prometheus metrics HTTP listener. Null disables metrics. |
| `metricsOpenFirewall` | `bool` | `false` | Open the metrics port in the firewall. Only effective when `metricsPort` is set. |

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

## Prometheus Metrics

When `metricsPort` is set, the agent starts a Prometheus HTTP listener on that port. Null (the default) disables the listener.

Metrics exposed:

| Metric | Description |
|--------|-------------|
| `nixfleet_agent_state` | Current phase of the deploy cycle (idle, checking, fetching, applying, verifying, reporting, rolling_back) encoded as a label |
| `nixfleet_agent_poll_duration_seconds` | Duration of the last poll cycle |
| `nixfleet_agent_last_poll_timestamp_seconds` | Unix timestamp of the last completed poll |
| `nixfleet_agent_health_check_duration_seconds` | Duration of the last health check run |
| `nixfleet_agent_health_check_status` | Result of the last health check (1 = healthy, 0 = unhealthy) |
| `nixfleet_agent_generation_info` | Nix store path of the current active generation (as a label) |

Metrics are served in the standard Prometheus text format at `GET /metrics`.

Example configuration:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  metricsPort = 9101;
  metricsOpenFirewall = true;
};
```

## Systemd service

The agent runs as a privileged root systemd service:

| Setting | Value |
|---------|-------|
| Target | `multi-user.target` |
| After | `network-online.target`, `nix-daemon.service` |
| Restart | `always` (30s delay) |
| StateDirectory | `nixfleet` |
| NoNewPrivileges | `true` |
| PATH | `${config.nix.package}/bin:${pkgs.systemd}/bin` |
| Environment | `XDG_CACHE_HOME=/var/lib/nixfleet/.cache` |

**Hardening rationale.** The agent runs `switch-to-configuration` as a subprocess, which needs full system access (`/dev`, `/home`, cgroups, kernel modules). Sandboxing (e.g. `PrivateDevices`, `ProtectHome`) would break these operations. The threat model is equivalent to `sudo nixos-rebuild switch` as a daemon. `NoNewPrivileges = true` is kept to prevent setuid escalation.

- `nix` is in `PATH` for `nix copy` and `nix path-info`.
- `XDG_CACHE_HOME` points into the state directory so nix metadata cache persists on impermanent hosts.

Health check configuration is written to `/etc/nixfleet/health-checks.json` and passed via `--health-config`.

On impermanent hosts, `/var/lib/nixfleet` is automatically persisted (including the XDG cache subdirectory).
