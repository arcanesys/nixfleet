# Rollouts

A rollout is a fleet-wide deployment coordinated by the control plane. Instead of pushing new code to every machine at once and hoping for the best, rollouts deploy in batches with health-check gates between each batch. If something breaks, the rollout pauses or reverts automatically.

Every rollout targets a **release** - an immutable CP-managed manifest mapping each host to its built Nix store path. This enables per-host deployment in heterogeneous fleets where every machine's closure is different (different hardware, hostSpec, modules, certificates). You create a release once (`nixfleet release create`), then trigger one or more rollouts against it.

## The two-step flow

```
nixfleet release create --push-to ssh://root@cache   # build + push + register
nixfleet deploy --release rel-abc123 --tags web --strategy canary --wait
```

Or use the convenience shorthand - `nixfleet deploy` with `--push-to` / `--copy` implicitly creates a release first:

```
nixfleet deploy --push-to ssh://root@cache --tags web --strategy canary --wait
```

Both forms do the same thing. The explicit form is useful when you want to deploy the same release multiple times (e.g., staging then prod, or rolling forward then back).

## Strategies

### All-at-once

Deploy to every targeted machine simultaneously. No batching, no gates. Suitable for dev/staging environments or non-critical updates.

```sh
nixfleet deploy --release rel-abc123 --tags staging --strategy all-at-once
```

### Canary

Deploy to a single machine first. If that machine passes health checks within the timeout, deploy to all remaining machines. Suitable for production environments where you want a quick smoke test.

```sh
nixfleet deploy --release rel-abc123 --tags prod --strategy canary \
  --health-timeout 120 --wait
```

This creates two batches: batch 0 with 1 machine, batch 1 with the rest.

### Staged

Define explicit batch sizes for fine-grained control. Batch sizes can be absolute numbers or percentages.

```sh
nixfleet deploy --release rel-abc123 --tags prod --strategy staged \
  --batch-size 1,25%,100% \
  --health-timeout 300 --wait
```

This creates three batches:
1. **Batch 0**: 1 machine (canary)
2. **Batch 1**: 25% of remaining machines
3. **Batch 2**: all remaining machines (100%)

## How rollouts work

1. **Create** - The CLI posts a rollout to the control plane with the `release_id`, target filter (tags or hosts), and strategy. The CP loads the release entries, intersects them with the target machine set (machines not in the release are skipped with a warning), randomizes the order, and splits them into batches.

2. **Execute batches** - The rollout executor (a background task in the CP) processes batches sequentially:
   - For each machine in the current batch, looks up the per-host store path from the release entries
   - Captures the machine's current generation into the batch's `previous_generations` map (for per-machine rollback)
   - Sets the desired generation on each machine via the internal `generations` table
   - Returns `poll_hint: 5` in the agent's next desired-generation response so agents react within seconds instead of waiting the full `pollInterval`
   - Agents poll, detect the mismatch, fetch the closure, apply, run health checks, and report back with their new `current_generation`

3. **Health gate** - The executor evaluates each machine's health by verifying TWO conditions:
   - The machine's latest report's `current_generation` matches the desired store path from the release entry (proves the agent actually applied the new generation)
   - A health report with `all_passed = true` has been received since the batch started

   This two-step gate prevents false-positive completion from stale health reports: a health report from a previous generation cannot count toward the new batch.

4. **Complete or fail** - When all batches succeed, the rollout status moves to `completed`. If a health gate fails, the rollout transitions to `paused` or `failed` depending on the `--on-failure` setting.

## Health gates

After each batch deploys, the control plane waits for agents to report health. The gate evaluates based on two parameters:

- **`--health-timeout`** (default: `300` seconds) - Maximum time to wait for health reports after a batch deploys. Machines that do not report within this window are marked as timed out. Set this higher than `pollInterval` so agents have time to notice the deploy (or rely on `poll_hint` to react within 5s).
- **`--failure-threshold`** (default: `0`) - Maximum number of unhealthy/timed-out machines before triggering the failure action. `0` means zero tolerance - any single failure pauses the rollout. Can be absolute (`"3"`) or a percentage of the batch (`"30%"`).

When the threshold is exceeded:

- **`--on-failure pause`** (default) - The rollout pauses. Investigate, fix the issue, then resume with `nixfleet rollout resume <id>`. Machines in the failed batch that did deploy are left in place (the agent already rolled back individually if its own health checks failed).
- **`--on-failure revert`** - The rollout fails and the CP reads each completed batch's `previous_generations` map, reverting every machine in those batches to the store path it was running before the rollout started. Each machine rolls back to its OWN previous state - not a single shared generation - which is the correct behavior for heterogeneous fleets.

## CLI flags

See [CLI reference - deploy](../../reference/cli.md#deploy) for the full flag list with defaults and descriptions.

## Monitoring rollouts

Stream progress in real time with `--wait`:

```sh
nixfleet deploy --release rel-abc123 --tags prod --strategy canary --wait
```

If `--on-failure pause` triggers, `--wait` exits immediately with an actionable message instead of blocking until timeout:

```
Rollout r-xxx paused: batch 1 health check failed (2/3 unhealthy)
  Resume with:  nixfleet rollout resume r-xxx
  Monitor with: nixfleet rollout status r-xxx --watch
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

For environments without a control plane (small fleets, bootstrapping, or air-gapped networks), the CLI can deploy directly over SSH without using a release:

```sh
nixfleet deploy --ssh --hosts "web*" --flake .
```

This builds each matching host's closure locally, copies it to the target via `nix-copy-closure`, and runs `switch-to-configuration switch`. No rollout orchestration, no release manifest, no health gates - just a direct push. Useful for initial bootstrap or quick one-off deploys.

## Worked example: canary deploy to production

**Step 1** - build all production hosts and register a release. If you use harmonia as a binary cache, `--push-to ssh://` copies the closures to the cache host's `/nix/store` where harmonia serves them immediately:

```sh
nixfleet release create \
  --flake . \
  --hosts 'web-*,db-*' \
  --push-to ssh://root@cache
```

Output includes the release ID, for example `rel-abc123-...`.

**Step 2** - deploy with canary strategy, 2-minute health timeout, auto-pause on failure:

```sh
nixfleet deploy \
  --release rel-abc123 \
  --tags prod,web \
  --strategy canary \
  --health-timeout 120 \
  --failure-threshold 1 \
  --on-failure pause \
  --wait
```

What happens:

1. The CP loads the release entries, filters by `prod` AND `web` tags, intersects with the release's host list (skipping any tagged machine not in the release), and randomizes the order.
2. **Batch 0**: 1 machine receives its per-host store path as desired. The CP starts returning `poll_hint=5` in the agent's desired-generation response.
3. Within ~5s, the agent polls, sees the mismatch, fetches the closure via `nix copy --from http://cache:5000`, runs `switch-to-configuration switch`, runs health checks, reports back.
4. The CP verifies the agent's report shows the new `current_generation` (not a stale report from before the deploy), then waits for a passing health report.
5. If healthy within 120s: **Batch 1** deploys to all remaining machines in parallel.
6. If unhealthy: the rollout pauses. The canary machine's agent has already rolled back locally. Run `nixfleet rollout status <id>` to investigate, then `nixfleet rollout resume <id>` or `nixfleet rollout cancel <id>`.

**Step 3** - same release, different environment:

```sh
# Same release, redeploy to a different subset with a different strategy
nixfleet deploy --release rel-abc123 --tags staging --strategy all-at-once --wait
```
