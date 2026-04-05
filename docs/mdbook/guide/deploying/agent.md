# Agent

The agent runs on each managed host as a systemd service. It polls the control plane for a desired generation, applies changes when a mismatch is detected, runs health checks, reports status, and automatically rolls back on failure.

## Enabling the agent

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  tags = ["web" "prod" "eu-west"];
  pollInterval = 300;
  healthInterval = 60;

  healthChecks = {
    systemd = [{ units = ["nginx.service" "postgresql.service"]; }];
    http = [{
      url = "http://localhost:8080/health";
      expectedStatus = 200;
      timeout = 3;
      interval = 5;
    }];
    command = [{
      name = "disk-space";
      command = "test $(df --output=pcent / | tail -1 | tr -d '% ') -lt 90";
      timeout = 5;
      interval = 10;
    }];
  };
};
```

## Agent options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the agent service |
| `controlPlaneUrl` | string | — (required) | URL of the control plane |
| `machineId` | string | `config.networking.hostName` | Machine identifier reported to the CP |
| `pollInterval` | int | `300` | Seconds between polls for desired generation |
| `cacheUrl` | string or null | `null` | Binary cache URL for `nix copy --from` |
| `dbPath` | string | `"/var/lib/nixfleet/state.db"` | SQLite state database path |
| `dryRun` | bool | `false` | Check and fetch but do not apply generations |
| `tags` | list of string | `[]` | Tags for grouping in fleet operations |
| `healthInterval` | int | `60` | Seconds between continuous health reports |

## State machine

The agent operates as an explicit state machine. Each poll cycle follows this flow:

```
Idle ──(poll timer)──→ Checking ──(mismatch)──→ Fetching ──→ Applying ──→ Verifying ──→ Reporting ──→ Idle
                          │                        │            │             │
                          │                        │            ↓             ↓
                          ↓                        ↓        RollingBack ← (failure)
                        Idle                     Idle            │
                     (up-to-date               (fetch            ↓
                      or error)                error)        Reporting
                                                                │
                                                                ↓
                                                              Idle
```

**Idle** — Waiting for the next poll interval. Continuous health reports fire during this state.

**Checking** — Queries the control plane for the desired generation and compares it to the current system profile. If they match, skips directly to Reporting with "up-to-date". On mismatch, transitions to Fetching.

**Fetching** — Resolves the closure using one of two paths depending on cache configuration:
- If a cache URL is available (per-generation `cache_url` from the CP, or the configured `cacheUrl`): fetches via `nix copy --from <cache_url> <store_path>`.
- If no cache URL is configured: verifies the store path exists locally via `nix path-info`. If the path is missing, the agent transitions to Idle and reports the error. In this mode the store path must be pre-pushed to the host via SSH or another out-of-band mechanism before the agent polls.

**Applying** — Runs `switch-to-configuration switch` to activate the new generation. If the switch command fails, transitions directly to RollingBack.

**Verifying** — Runs all configured health checks against the newly activated system. If every check passes, transitions to Reporting with success. If any check fails, transitions to RollingBack.

**RollingBack** — Switches back to the previous generation. The reason (apply failure or health check failure) is recorded and propagated to the report.

**Reporting** — Sends a status report to the control plane with the current generation, success/failure status, tags, and optionally a health report. Always transitions back to Idle.

## Health checks

Three types of health check are supported, all configured declaratively in Nix:

### Systemd units

Verify that critical systemd units are in the `active` state.

```nix
healthChecks.systemd = [{
  units = ["nginx.service" "postgresql.service"];
}];
```

### HTTP endpoints

Send a GET request and verify the response status code.

| Suboption | Type | Default | Description |
|-----------|------|---------|-------------|
| `url` | string | — (required) | URL to GET |
| `expectedStatus` | int | `200` | Expected HTTP status code |
| `timeout` | int | `3` | Timeout in seconds |
| `interval` | int | `5` | Check interval in seconds |

```nix
healthChecks.http = [{
  url = "http://localhost:3000/healthz";
  expectedStatus = 200;
  timeout = 5;
}];
```

### Custom commands

Run an arbitrary shell command. Exit code 0 means healthy.

| Suboption | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | string | — (required) | Check name (used in reports) |
| `command` | string | — (required) | Shell command to execute |
| `timeout` | int | `5` | Timeout in seconds |
| `interval` | int | `10` | Check interval in seconds |

```nix
healthChecks.command = [{
  name = "disk-space";
  command = "test $(df --output=pcent / | tail -1 | tr -d '% ') -lt 90";
  timeout = 5;
}];
```

## Continuous health reporting

Independent of the deployment cycle, the agent sends periodic health reports to the control plane at the `healthInterval` cadence (default: 60 seconds). These reports run only while the agent is idle (not mid-deployment) and include the results of all configured health checks.

The control plane uses these continuous reports to:
- Track fleet health over time
- Inform rollout health gates (a machine reporting unhealthy will affect batch success evaluation)
- Surface issues in `nixfleet status` output

## Prometheus Metrics

Enable the agent metrics listener by setting `metricsPort`:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  metricsPort = 9101;
  metricsOpenFirewall = true;
};
```

Scrape from Prometheus at `http://agent-host:9101/metrics`. See [Agent Options](../../reference/agent-options.md) for the full list of exposed metrics.

## Registration

On first health report, the control plane automatically registers the agent, setting it to `active` and syncing its tags. No manual registration step is required.

Auto-registration is gated by mTLS — only agents presenting a valid client certificate signed by the fleet CA can register. Admins can also pre-register machines via `POST /api/v1/machines/{id}/register` before agents come online.

## Tag Sync

Tags configured via `services.nixfleet-agent.tags` are sent in every health report and automatically synced to the control plane. No manual tag management needed — change the NixOS config, rebuild, and the CP picks up the new tags on the next report cycle.

To verify enrollment:

```sh
nixfleet machines list
```

To filter by tag:

```sh
nixfleet machines list --tag prod
```

## Persistence

Agent state is stored in a SQLite database at `dbPath`. On impermanent NixOS hosts, the module automatically persists `/var/lib/nixfleet` across reboots.

## Security

The agent supports mTLS for control plane communication via CLI flags / environment variables:

| Flag | Env var | Description |
|------|---------|-------------|
| `--client-cert` | `NIXFLEET_CLIENT_CERT` | Client certificate PEM file |
| `--client-key` | `NIXFLEET_CLIENT_KEY` | Client private key PEM file |
| `--allow-insecure` | `NIXFLEET_ALLOW_INSECURE` | Allow HTTP (dev only, default: false) |

The systemd service is hardened with `NoNewPrivileges`, `ProtectHome`, `PrivateTmp`, `PrivateDevices`, and restricted filesystem access.
