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

**Releases** are immutable manifests mapping each host to its built Nix store path. A release captures "what the flake means for each host at a point in time". Created via `nixfleet release create`, they can be inspected, diffed, listed, and referenced by rollouts multiple times (e.g., staging then prod, or rollback to a previous release). See [CLI reference](../../reference/cli.md#release-create).

**Rollouts** coordinate fleet-wide deployments across batches with health gates between each batch. Every rollout references a release — the CP resolves each target machine's store path from the release entries at batch execution time. See [Rollouts](rollouts.md) for details.

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

The control plane uses two independent auth layers: the TLS layer (authentication) and the API layer (authorization).

| Layer | Mechanism | Who | Purpose |
|-------|-----------|-----|---------|
| **TLS** | mTLS client certs | Agents + admin clients | Authenticate the connection |
| **API** | API keys | Admin clients only | Authorize specific operations |

### Configuration

TLS and mTLS are configured via:

| Flag | Env var | Description |
|------|---------|-------------|
| `--tls-cert` | `NIXFLEET_CP_TLS_CERT` | Path to server certificate PEM (enables HTTPS) |
| `--tls-key` | `NIXFLEET_CP_TLS_KEY` | Path to server private key PEM |
| `--client-ca` | `NIXFLEET_CP_CLIENT_CA` | Path to client CA PEM (enables required mTLS for all connections) |

Both `--tls-cert` and `--tls-key` must be provided together to enable HTTPS.

When `--client-ca` is set, **all** TLS connections must present a client certificate signed by that CA:
- **Agents** authenticate via their client certificate; no API key needed. Agents are auto-registered on their first health report — any client with a valid fleet cert can register as a machine.
- **Admin clients** (CLI, REST API) must present both a client certificate AND an API key (defense-in-depth)

API keys are passed via the `Authorization: Bearer <key>` header.

### Bootstrap

On first deployment, create the initial admin key via the bootstrap endpoint (only works when no keys exist):

```bash
curl -X POST https://cp-host:8080/api/v1/keys/bootstrap \
  --cacert fleet-ca.pem --cert client-cert.pem --key client-key.pem \
  -H 'Content-Type: application/json' -d '{"name":"admin"}'
# Returns: {"key":"nfk-...","name":"admin","role":"admin"}
```

Save the returned key — it's only shown once. Subsequent calls return 409 Conflict.

> **Production recommendation:** Always enable TLS. When managing a fleet with agent certificates, set `--client-ca` to require mTLS from all clients. Admin clients must have access to both their client certificate and an API key.

## Persistence

State is stored in a single SQLite database at `dbPath`. On impermanent NixOS hosts, the module automatically persists `/var/lib/nixfleet-cp` across reboots.

A background task cleans up health reports older than 24 hours to prevent unbounded database growth.
