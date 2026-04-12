# Control Plane

The control plane is a lightweight HTTP server that coordinates fleet deployments. It provides:

- **Machine registry** — agents auto-register on first report; machines are tracked with tags and lifecycle states
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

See [Control Plane Options](../../reference/control-plane-options.md) for the full option reference including TLS, metrics, and systemd service details.

## Verify

```sh
systemctl status nixfleet-control-plane
curl http://localhost:8080/health
```

## What it manages

**Machines** auto-register when the agent sends its first report to the control plane. Each machine has:

- A unique ID (defaults to hostname)
- Tags for grouping (`web`, `prod`, `eu-west`, etc.)
- A lifecycle state (see [TECHNICAL.md](../../../../TECHNICAL.md) for the full transition graph)

**Releases** are immutable manifests mapping each host to its built Nix store path. A release captures "what the flake means for each host at a point in time". Created via `nixfleet release create`, they can be inspected, diffed, listed, and referenced by rollouts multiple times (e.g., staging then prod, or rollback to a previous release). See [CLI reference](../../reference/cli.md#release-create).

**Rollouts** coordinate fleet-wide deployments across batches with health gates between each batch. Every rollout references a release — the CP resolves each target machine's store path from the release entries at batch execution time. See [Rollouts](rollouts.md) for details.

**Audit events** record every mutation (deployment, rollback, tag change, lifecycle transition) with actor, timestamp, and detail. Query them with:

```sh
curl http://localhost:8080/api/v1/audit           # JSON
curl http://localhost:8080/api/v1/audit/export     # CSV
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
| **API** | API keys (role-gated) | Admin clients only | Authorize specific operations |

API keys have one of three roles: `admin` (full access), `deploy` (create releases and rollouts), `readonly` (read-only). The bootstrap endpoint creates an `admin` key.

### Configuration

```nix
services.nixfleet-control-plane = {
  enable = true;
  tls.cert = "/run/secrets/cp-cert.pem";      # enables HTTPS
  tls.key = "/run/secrets/cp-key.pem";
  tls.clientCa = "/run/secrets/fleet-ca.pem";  # enables required mTLS
};
```

When `tls.clientCa` is set, **all** connections must present a valid client certificate:
- **Agents** authenticate via client cert alone (no API key)
- **Admin clients** require both a client cert AND an API key (`Authorization: Bearer <key>`)

See [Control Plane Options](../../reference/control-plane-options.md) for full TLS option details.

### Bootstrap

On first deployment, create the initial admin key via the bootstrap endpoint (only works when no keys exist):

```bash
curl -X POST https://cp-host:8080/api/v1/keys/bootstrap \
  --cacert fleet-ca.pem --cert client-cert.pem --key client-key.pem \
  -H 'Content-Type: application/json' -d '{"name":"admin"}'
# Returns: {"key":"nfk-...","name":"admin","role":"admin"}
```

Save the returned key — it's only shown once. Subsequent calls return 409 Conflict.

> **Production recommendation:** Always enable TLS. Set `tls.clientCa` to require mTLS from all clients. Admin clients need both a client certificate and an API key.

## Persistence

State is stored in a single SQLite database at `dbPath`. On impermanent NixOS hosts, the module automatically persists `/var/lib/nixfleet-cp` across reboots.

A background task cleans up health reports older than 24 hours to prevent unbounded database growth.
