# Rollouts

A rollout is a fleet-wide deployment coordinated by the control plane. Instead of pushing a new generation to every machine at once and hoping for the best, rollouts deploy in batches with health-check gates between each batch. If something breaks, the rollout pauses or reverts automatically.

## Strategies

### All-at-once

Deploy to every targeted machine simultaneously. No batching, no gates. Suitable for dev/staging environments or non-critical updates.

```sh
nixfleet deploy --tag staging --strategy all-at-once \
  --generation /nix/store/abc123-nixos-system
```

### Canary

Deploy to a single machine first. If that machine passes health checks within the timeout, deploy to all remaining machines. Suitable for production environments where you want a quick smoke test.

```sh
nixfleet deploy --tag prod --strategy canary \
  --generation /nix/store/abc123-nixos-system \
  --health-timeout 120 --wait
```

This creates two batches: batch 0 with 1 machine, batch 1 with the rest.

### Staged

Define explicit batch sizes for fine-grained control. Batch sizes can be absolute numbers or percentages.

```sh
nixfleet deploy --tag prod --strategy staged \
  --batch-size 1,25%,100% \
  --generation /nix/store/abc123-nixos-system \
  --health-timeout 300 --wait
```

This creates three batches:
1. **Batch 0**: 1 machine (canary)
2. **Batch 1**: 25% of remaining machines
3. **Batch 2**: all remaining machines (100%)

## How rollouts work

1. **Create** — The CLI posts a rollout to the control plane with the target generation, tag filter, and strategy. The CP filters machines by tags (only `active` lifecycle machines are included), randomizes the order, and splits them into batches.

2. **Execute batches** — The rollout executor (a background task in the CP) processes batches sequentially:
   - Sets the desired generation on each machine in the current batch
   - Agents poll, detect the mismatch, fetch the closure, apply, run health checks, and report back
   - The CP waits up to `--health-timeout` seconds for all machines in the batch to report

3. **Health gate** — After each batch, the CP evaluates health reports:
   - If all machines report healthy: advance to the next batch
   - If failures exceed `--failure-threshold`: trigger the `--on-failure` action

4. **Complete or fail** — When all batches succeed, the rollout status moves to `completed`. If a health gate fails, the rollout transitions to `paused` or `failed` depending on the `--on-failure` setting.

## Health gates

After each batch deploys, the control plane waits for agents to report health. The gate evaluates based on two parameters:

- **`--health-timeout`** (default: `300` seconds) — Maximum time to wait for health reports after a batch deploys. Machines that do not report within this window are marked as timed out.
- **`--failure-threshold`** (default: `1`) — Maximum number of unhealthy/timed-out machines before triggering the failure action.

When the threshold is exceeded:

- **`--on-failure pause`** (default) — The rollout pauses. Investigate, fix the issue, then resume with `nixfleet rollout resume <id>`. Machines in the failed batch that did deploy are left in place (the agent already rolled back individually if its own health checks failed).
- **`--on-failure revert`** — The rollout fails and the CP sets the desired generation back to the previous value for all affected machines, triggering agent-level rollbacks.

## CLI flags

All flags for `nixfleet deploy`:

| Flag | Default | Description |
|------|---------|-------------|
| `--tag <TAG>` | — | Target machines by tag (repeatable) |
| `--generation <PATH>` | — (required for rollout mode) | Nix store path of the built closure |
| `--strategy <STRATEGY>` | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size <SIZES>` | — | Comma-separated batch sizes (e.g., `1,25%,100%`) |
| `--failure-threshold <N>` | `1` | Max failures before pausing/reverting |
| `--on-failure <ACTION>` | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | `300` | Seconds to wait for health reports per batch |
| `--wait` | `false` | Stream rollout progress to stdout |
| `--dry-run` | `false` | Build closures and show plan without deploying |
| `--flake <REF>` | `.` | Flake reference for builds |
| `--ssh` | `false` | SSH fallback mode (bypasses control plane) |
| `--hosts <PATTERN>` | `*` | Host glob pattern (SSH mode only) |

Global flags:

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--control-plane-url` | `NIXFLEET_CP_URL` | `http://localhost:8080` | Control plane URL |
| `--api-key` | `NIXFLEET_API_KEY` | — | API key for authentication |

## Monitoring rollouts

Stream progress in real time with `--wait`:

```sh
nixfleet deploy --tag prod --strategy canary \
  --generation /nix/store/abc123-nixos-system --wait
```

List rollouts:

```sh
nixfleet rollout list
nixfleet rollout list --status running
nixfleet rollout list --status paused
```

Inspect a specific rollout with per-batch and per-machine detail:

```sh
nixfleet rollout status <rollout-id>
```

## Managing rollouts

Resume a paused rollout (after investigating and fixing the issue):

```sh
nixfleet rollout resume <rollout-id>
```

Cancel a rollout (stops further batches, leaves already-deployed machines as-is):

```sh
nixfleet rollout cancel <rollout-id>
```

## SSH fallback

For environments without a control plane (small fleets, bootstrapping, or air-gapped networks), the CLI can deploy directly over SSH:

```sh
nixfleet deploy --ssh --hosts "web*" --flake .
```

This builds each matching host's closure locally, copies it to the target via `nix copy`, and runs `switch-to-configuration switch`. No rollout orchestration, no health gates — just a direct push.

## Worked example: canary deploy to production

Build the closure for your fleet:

```sh
nix build .#nixosConfigurations.web-01.config.system.build.toplevel
GENERATION=$(readlink -f result)
```

Deploy with canary strategy, 2-minute health timeout, auto-pause on failure:

```sh
nixfleet deploy \
  --tag prod --tag web \
  --strategy canary \
  --generation "$GENERATION" \
  --health-timeout 120 \
  --failure-threshold 1 \
  --on-failure pause \
  --wait
```

What happens:

1. The CP selects all machines tagged `prod` AND `web`, randomizes the order
2. **Batch 0**: 1 machine receives the new generation. Its agent polls, fetches, applies, runs health checks, reports back.
3. The CP waits up to 120 seconds for a healthy report.
4. If healthy: **Batch 1** deploys to all remaining machines.
5. If unhealthy: the rollout pauses. The canary machine's agent has already rolled back locally. Run `nixfleet rollout status <id>` to investigate, then `nixfleet rollout resume <id>` or `nixfleet rollout cancel <id>`.
