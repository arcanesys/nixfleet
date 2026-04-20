# Control Plane Options

All options under `services.nixfleet-control-plane`. The module is auto-included by mkHost and disabled by default.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet control plane server. |
| `listen` | `str` | `"0.0.0.0:8080"` | Address and port to listen on. |
| `dbPath` | `str` | `"/var/lib/nixfleet-cp/state.db"` | Path to the SQLite state database. |
| `openFirewall` | `bool` | `false` | Open the control plane port in the firewall. The port is parsed from the `listen` value. |
| `tls.cert` | `nullOr str` | `null` | Path to TLS certificate PEM file. Enables HTTPS when set (requires `tls.key`). Example: `"/run/secrets/cp-cert.pem"`. |
| `tls.key` | `nullOr str` | `null` | Path to TLS private key PEM file. Example: `"/run/secrets/cp-key.pem"`. |
| `tls.clientCa` | `nullOr str` | `null` | Path to client CA PEM file. When set, **all** TLS connections must present a valid client certificate signed by this CA (required mTLS). Admin clients must present both a client cert and an API key. Example: `"/run/secrets/fleet-ca.pem"`. |

## Prometheus Metrics

The control plane exposes a `GET /metrics` endpoint on its listen address. No separate port or additional configuration is required - the endpoint is always available when the service is running.

No authentication is required for `/metrics` (same as `/health`). Restrict access at the network level if needed.

Metrics exposed:

| Metric | Description |
|--------|-------------|
| `nixfleet_fleet_size` | Total number of registered machines |
| `nixfleet_machines_by_lifecycle` | Machine count grouped by lifecycle state (label: `lifecycle`) |
| `nixfleet_machine_last_seen_timestamp_seconds` | Unix timestamp of each machine's last report (label: `machine_id`) |
| `nixfleet_http_requests_total` | HTTP request count by method, path, and status code |
| `nixfleet_http_request_duration_seconds` | HTTP request latency histogram |
| `nixfleet_rollouts_total` | Rollout count by status (label: `status`) |
| `nixfleet_rollouts_active` | Number of currently active rollouts (created, running, or paused) |

Example:

```sh
curl http://localhost:8080/metrics
```

## Systemd service

| Setting | Value |
|---------|-------|
| Target | `multi-user.target` |
| After | `network-online.target` |
| Restart | `always` (10s delay) |
| StateDirectory | `nixfleet-cp` |
| NoNewPrivileges | `true` |
| ProtectHome | `true` |
| PrivateTmp | `true` |
| PrivateDevices | `true` |
| ProtectKernelTunables | `true` |
| ProtectKernelModules | `true` |
| ProtectControlGroups | `true` |
| ReadWritePaths | `/var/lib/nixfleet-cp` |

## Example

```nix
services.nixfleet-control-plane = {
  enable = true;
  listen = "0.0.0.0:8080";
  openFirewall = true;
};
```

On impermanent hosts, `/var/lib/nixfleet-cp` is automatically persisted.
