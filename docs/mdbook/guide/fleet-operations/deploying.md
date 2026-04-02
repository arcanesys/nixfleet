# Deploying to Your Fleet

Once the control plane is running and agents are enrolled, use the CLI to deploy configurations across your fleet.

## Basic Deploy

```sh
# Deploy to all machines
nixfleet deploy --flake .

# Deploy to machines with a specific tag
nixfleet deploy --flake . --tag production

# Dry run — see what would happen
nixfleet deploy --flake . --tag web --dry-run
```

## Rollout Strategies

### all-at-once (default)

Deploy to all targeted machines simultaneously. Fast, but a bad generation affects the entire fleet.

```sh
nixfleet deploy --tag production --strategy all-at-once
```

### canary

Deploy to a single machine first. If health checks pass, deploy to the rest.

```sh
nixfleet deploy --tag production --strategy canary --wait
```

### staged

Deploy in explicit batches with health check gates between each:

```sh
nixfleet deploy --tag production --strategy staged \
  --batch-size 1,25%,100% \
  --failure-threshold 1 \
  --on-failure revert \
  --health-timeout 300 \
  --wait
```

This deploys to 1 machine, waits for health checks, then 25% of the remaining fleet, then the rest. If any batch exceeds the failure threshold, the rollout is automatically reverted.

## Monitoring a Rollout

```sh
# Watch a rollout in real time (--wait streams progress)
nixfleet deploy --tag web --strategy canary --wait

# Check status of an active rollout
nixfleet rollout status <rollout-id>

# List all rollouts
nixfleet rollout list
nixfleet rollout list --status running
```

## Managing Rollouts

```sh
# Pause a rollout (automatic on failure if --on-failure pause)
# Resume a paused rollout
nixfleet rollout resume <rollout-id>

# Cancel a rollout entirely
nixfleet rollout cancel <rollout-id>
```

## How Health Checks Drive Rollouts

After each batch is deployed:

1. The control plane waits up to `--health-timeout` seconds
2. Agents run their configured checks (systemd, HTTP, command)
3. Agents report health status to the control plane
4. If failures exceed `--failure-threshold`:
   - `--on-failure pause` — rollout pauses, operator investigates
   - `--on-failure revert` — agents roll back, rollout marked as reverted
5. If all healthy — proceed to the next batch

## Fleet Status

```sh
# Overview of all machines and their status
nixfleet status

# JSON output for scripting
nixfleet status --json
```

## Rollback

```sh
# Roll back a specific host
nixfleet rollback --host web-01

# Roll back to a specific generation
nixfleet rollback --host web-01 --generation /nix/store/abc...
```

## SSH Fallback

For environments without a control plane, the CLI can deploy directly via SSH:

```sh
nixfleet deploy --ssh --hosts "web*" --flake .
nixfleet rollback --host web-01 --ssh
```

This uses `nixos-rebuild` over SSH — no agent or control plane required.

## Next Steps

- [CLI Reference](../../cli/README.md) — full command documentation
- [Agent Health Checks](../../scopes/nixfleet-agent.md#health-checks) — configuring checks
- [Control Plane](../../scopes/nixfleet-control-plane.md) — service module details
