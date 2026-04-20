# Agent

The agent runs on each managed host as a systemd service. It polls the control plane for a desired generation, applies changes when a mismatch is detected, runs health checks, reports status, and automatically rolls back on failure.

## Enabling the agent

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  tags = ["web" "prod" "eu-west"];
  pollInterval = 60;
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

See [Agent Options](../../reference/agent-options.md) for the full option reference including TLS, metrics, health checks, and systemd service details.

## Deploy cycle

On every poll tick the agent runs a single sequential deploy cycle (`run_deploy_cycle`) to completion - no cooperative state machine, no interruptible transitions:

1. **Check** - `GET /api/v1/machines/<id>/desired-generation` returns `{hash, cache_url, poll_hint}`. If `hash` matches `/run/current-system`, the cycle reports "up-to-date" and returns. If `poll_hint` is set (active rollout), the next poll is scheduled at that shorter interval.
2. **Fetch** - if the generation differs, the agent runs `nix copy --from <cache_url> <hash>`. With no cache URL, it falls back to `nix path-info` to verify the path was pre-pushed out-of-band.
3. **Apply** - runs `<hash>/bin/switch-to-configuration switch` as a subprocess. The agent is a privileged root service - sandboxing is minimal because `switch-to-configuration` needs access to `/dev`, `/home`, `/root`, cgroups, and kernel modules to do its job.
4. **Verify** - runs all configured health checks. If any fail, the agent transitions to rollback.
5. **Report** - posts a `Report` to the CP with `current_generation`, `success`, and `message`. The executor uses `current_generation` to verify the machine has actually applied the new generation before accepting health-gated completion.

On any failure (network, fetch, apply, or verify), the cycle returns `PollOutcome::Failed` and the main loop reschedules the next poll to `retryInterval` (30s by default) instead of the full `pollInterval`. This handles bootstrap races (agent polls before the CP has a release), transient network failures, and flaky fetches.

**Periodic health reports** run on a separate `healthInterval` tick (default 60s) independent of the deploy cycle. The executor only counts a health report toward batch completion when the machine's `current_generation` matches the desired store path from the release entry.

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
| `url` | string | - (required) | URL to GET |
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
| `name` | string | - (required) | Check name (used in reports) |
| `command` | string | - (required) | Shell command to execute |
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

The agent sends periodic health reports at `healthInterval` (default: 60s), independent of deploy cycles. The CP uses these to track fleet health, evaluate rollout health gates, and surface issues in `nixfleet status`.

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

## Registration & tags

Agents auto-register on first report (gated by mTLS). Tags from `services.nixfleet-agent.tags` sync on every report - change the NixOS config, rebuild, and the CP picks up the new tags automatically. Admins can pre-register machines via `nixfleet machines register <id>`.

```sh
nixfleet machines list              # verify enrollment
nixfleet machines list --tags prod  # filter by tag
```

## Persistence

Agent state is stored in a SQLite database at `dbPath`. On impermanent NixOS hosts, the module automatically persists `/var/lib/nixfleet` across reboots.

## Security

Configure mTLS via the NixOS module options `tls.clientCert` and `tls.clientKey`. Set `allowInsecure = true` for dev-only HTTP mode.

The systemd service runs without sandboxing because `switch-to-configuration` needs full system access. See [Agent Options - Systemd service](../../reference/agent-options.md#systemd-service) for the full hardening rationale.
