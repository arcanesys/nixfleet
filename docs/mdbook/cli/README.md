# CLI Reference

The `nixfleet` CLI manages fleet operations through the control plane. It communicates with the control plane's REST API.

## Global Options

| Option | Env | Default | Description |
|--------|-----|---------|-------------|
| `--control-plane-url` | `NIXFLEET_CP_URL` | `http://localhost:8080` | Control plane URL |
| `--api-key` | `NIXFLEET_API_KEY` | (empty) | API key for authentication |

## Commands

### deploy

Deploy configurations to fleet hosts. Supports both SSH direct mode and control plane rollouts.

```sh
# SSH mode — push directly to hosts
nixfleet deploy --ssh --hosts "web*" --flake .

# Rollout mode — staged deployment via control plane
nixfleet deploy --tag production --strategy canary --wait

# Canary with custom batch sizes
nixfleet deploy --tag web --strategy staged --batch-size 1,25%,100% \
  --failure-threshold 1 --on-failure revert --health-timeout 300 --wait
```

| Option | Default | Description |
|--------|---------|-------------|
| `--hosts` | `*` | Host pattern (glob-style, SSH mode) |
| `--flake` | `.` | Flake reference |
| `--ssh` | false | SSH fallback mode (bypass control plane) |
| `--dry-run` | false | Show what would happen without applying |
| `--tag` | -- | Target tag(s) for rollout (repeatable) |
| `--strategy` | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size` | -- | Batch sizes (comma-separated, e.g. `1,25%,100%`) |
| `--failure-threshold` | `1` | Maximum failures before pausing/reverting |
| `--on-failure` | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout` | `300` | Health check timeout in seconds |
| `--wait` | false | Wait and stream rollout progress |
| `--generation` | -- | Store path hash (skip nix build) |

### status

Show fleet status from the control plane.

```sh
nixfleet status
nixfleet status --json
```

### rollback

Rollback a host to a previous generation.

```sh
nixfleet rollback --host web-01
nixfleet rollback --host web-01 --generation /nix/store/abc...
nixfleet rollback --host web-01 --ssh  # via SSH instead of CP
```

### rollout

Manage active rollouts.

```sh
nixfleet rollout list
nixfleet rollout list --status running
nixfleet rollout status <rollout-id>
nixfleet rollout resume <rollout-id>
nixfleet rollout cancel <rollout-id>
```

### machines

Manage machines and tags in the control plane registry.

```sh
nixfleet machines list
nixfleet machines list --tag production
nixfleet machines tag <machine-id> production eu-west
nixfleet machines untag <machine-id> eu-west
```

### host

Scaffold and provision new hosts.

```sh
# Generate configs for a new host
nixfleet host add --hostname web-03 --platform x86_64-linux

# Provision via nixos-anywhere
nixfleet host provision --hostname web-03 --target root@192.168.1.53
```

## Rollout Strategies

### all-at-once

Deploy to all targeted machines simultaneously. Fast but risky — a bad generation affects the entire fleet.

### canary

Deploy to a single machine first. If health checks pass, proceed to the rest. Default batch sizes: `1, 100%`.

### staged

Deploy in explicit batches. Specify sizes with `--batch-size`:

```sh
# 1 machine, then 25%, then the rest
nixfleet deploy --strategy staged --batch-size 1,25%,100% --tag production
```

Between batches, the control plane waits for health checks. If failures exceed `--failure-threshold`, the rollout pauses (or reverts if `--on-failure revert`).

## Health Check Integration

Rollout success depends on agent health checks. After each batch:

1. Control plane waits up to `--health-timeout` seconds
2. Agents run their configured health checks (systemd, HTTP, command)
3. Agents report health status to the control plane
4. If failures exceed threshold → pause or revert
5. If all healthy → proceed to next batch

See [Agent Health Checks](../scopes/nixfleet-agent.md#health-checks) for configuring checks.
