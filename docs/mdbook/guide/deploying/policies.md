# Rollout Policies

A rollout policy is a named preset of rollout parameters stored on the control plane. Policies capture a deployment strategy — batch sizes, failure thresholds, health timeouts — without being bound to any specific set of machines or a generation. Any deploy command can reference a policy by name.

## Creating a policy

```sh
nixfleet policy create \
  --name canary-web \
  --strategy canary \
  --failure-threshold 1 \
  --on-failure pause \
  --health-timeout 120
```

```sh
nixfleet policy create \
  --name staged-prod \
  --strategy staged \
  --batch-size 1,25%,100% \
  --failure-threshold 2 \
  --on-failure revert \
  --health-timeout 300
```

List and inspect policies:

```sh
nixfleet policy list
nixfleet policy get canary-web
```

Update a policy when your rollout requirements change:

```sh
nixfleet policy update canary-web --health-timeout 180
```

Delete a policy that is no longer needed:

```sh
nixfleet policy delete canary-web
```

## Using a policy in a deploy

Pass `--policy <NAME>` to a deploy command to apply the policy's values as defaults:

```sh
nixfleet deploy \
  --release rel-abc123 \
  --tag prod --tag web \
  --policy canary-web \
  --wait
```

The deploy inherits the strategy, batch sizes, failure threshold, on-failure action, and health timeout from the policy. Any flag you pass explicitly on the command line overrides the policy value.

## Policy resolution

When `--policy` is given, the control plane resolves the policy at rollout creation time:

1. The CLI sends the rollout request with the policy name and any explicit flag values.
2. The CP fetches the named policy.
3. For each rollout parameter, the explicit value wins if provided; otherwise the policy value is used; otherwise the built-in default applies.

This means you can use a policy as a baseline and override individual parameters per deploy:

```sh
# Use the staged-prod policy but extend the health timeout for this deploy
nixfleet deploy \
  --release rel-abc123 \
  --tag prod \
  --policy staged-prod \
  --health-timeout 600
```

## Scheduled rollouts

Add `--schedule-at` to any rollout deploy to schedule it for a future time. The value must be an ISO 8601 datetime string in UTC.

```sh
nixfleet deploy \
  --release rel-abc123 \
  --tag prod \
  --policy staged-prod \
  --schedule-at "2026-04-06T03:00:00Z"
```

The control plane stores the scheduled rollout and triggers it at the specified time. The deploy command returns immediately with the schedule ID.

Policies and scheduled rollouts work together: the policy values are resolved at trigger time, not at scheduling time. If you update the policy between scheduling and the trigger, the updated values apply.

List and manage scheduled rollouts:

```sh
# List all scheduled rollouts
nixfleet schedule list

# List only pending schedules
nixfleet schedule list --status pending

# Cancel a scheduled rollout before it triggers
nixfleet schedule cancel <schedule-id>
```

Schedule statuses:

| Status | Meaning |
|--------|---------|
| `pending` | Waiting to trigger |
| `triggered` | Rollout has been created and is running |
| `cancelled` | Cancelled before triggering |

## Rollout event history

Every rollout records a timeline of state transitions as events. Events capture what happened and when: batch started, batch completed, health gate passed or failed, rollout paused, resumed, cancelled, or completed.

View the event history for a rollout via `nixfleet rollout status`:

```sh
nixfleet rollout status <rollout-id>
```

The output includes per-batch and per-machine detail alongside the event timeline. This is the primary tool for investigating a paused or failed rollout.

## Binary cache

Every release records a `cache_url` (either from the `--push-to` URL used to create it, or an explicit `--cache-url` override). The CP returns this to each agent along with the desired generation, so agents automatically pull from the right cache without any extra configuration.

When you need to override the cache URL per-deploy (e.g., same release, different cache), pass `--cache-url`:

```sh
nixfleet deploy \
  --release rel-abc123 \
  --tag prod \
  --policy canary-web \
  --cache-url http://cache.internal:5000 \
  --wait
```

## Worked example: scheduled canary deploy

Build and register a release:

```sh
nixfleet release create \
  --hosts 'web-*' \
  --push-to ssh://root@cache
# Output: Release rel-abc123 created (12 hosts)
```

Create a policy for web production deploys:

```sh
nixfleet policy create \
  --name web-prod \
  --strategy canary \
  --failure-threshold 1 \
  --on-failure pause \
  --health-timeout 120
```

Schedule the deploy for a maintenance window:

```sh
nixfleet deploy \
  --release rel-abc123 \
  --tag prod --tag web \
  --policy web-prod \
  --schedule-at "2026-04-06T03:00:00Z"
```

Verify the schedule:

```sh
nixfleet schedule list --status pending
```

Cancel if needed before the window:

```sh
nixfleet schedule cancel <schedule-id>
```
