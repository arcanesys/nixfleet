# Control Plane

The control plane is a lightweight HTTP server that coordinates fleet deployments. It provides:

- **Machine registry** — agents auto-register on first poll; machines are tracked with tags and lifecycle states
- **Rollout orchestration** — staged, canary, and all-at-once deployment strategies with health-check gates
- **Tag storage** — group machines by role, environment, or any arbitrary label
- **Deployment audit log** — every action (deploy, rollback, tag change, lifecycle transition) is recorded
- **REST API** — all operations available programmatically at `/api/v1/`

## Enabling the service

```nix
services.nixfleet-control-plane = {
  enable = true;
  listen = "0.0.0.0:8080";
  dbPath = "/var/lib/nixfleet-cp/state.db";
  openFirewall = true;
};
```

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the control plane service |
| `listen` | string | `"0.0.0.0:8080"` | Address and port to bind |
| `dbPath` | string | `"/var/lib/nixfleet-cp/state.db"` | SQLite database path |
| `openFirewall` | bool | `false` | Open the listen port in the NixOS firewall |

## Verify

```sh
systemctl status nixfleet-control-plane
curl http://localhost:8080/health
```

## What it manages

**Machines** auto-register when their agent first polls the control plane. Each machine has:

- A unique ID (defaults to hostname)
- Tags for grouping (`web`, `prod`, `eu-west`, etc.)
- A lifecycle state: `pending` → `provisioning` → `active` ⇄ `maintenance` → `decommissioned`
- A desired generation (the Nix store path the agent should converge to)

**Rollouts** coordinate fleet-wide deployments across batches with health gates between each batch. See [Rollouts](rollouts.md) for details.

**Audit events** record every mutation (deployment, rollback, tag change, lifecycle transition) with actor, timestamp, and detail. Query them with:

```sh
curl http://localhost:8080/api/v1/audit
```

## Monitoring

The `/metrics` endpoint is available on the CP's listen address with no extra configuration. It is always active when the service is running.

Add a scrape target to your Prometheus configuration:

```yaml
scrape_configs:
  - job_name: nixfleet-control-plane
    static_configs:
      - targets: ["fleet.example.com:8080"]
```

See [Control Plane Options](../../reference/control-plane-options.md) for the full list of exposed metrics.

## Security

The control plane supports TLS and mutual TLS (mTLS) via command-line flags / environment variables:

| Flag | Env var | Description |
|------|---------|-------------|
| `--tls-cert` | `NIXFLEET_CP_TLS_CERT` | Path to server certificate PEM (enables HTTPS) |
| `--tls-key` | `NIXFLEET_CP_TLS_KEY` | Path to server private key PEM |
| `--client-ca` | `NIXFLEET_CP_CLIENT_CA` | Path to client CA PEM (enables mTLS) |

Both `--tls-cert` and `--tls-key` must be provided together. When `--client-ca` is also set, only agents presenting a certificate signed by that CA can connect.

The CLI supports API key authentication via `--api-key` / `NIXFLEET_API_KEY`.

> **Production recommendation:** Always enable TLS. Use mTLS for agent-to-CP communication and API keys for CLI-to-CP communication.

## Persistence

State is stored in a single SQLite database at `dbPath`. On impermanent NixOS hosts, the module automatically persists `/var/lib/nixfleet-cp` across reboots.

A background task cleans up health reports older than 24 hours to prevent unbounded database growth.
