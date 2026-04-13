# Agent Self-Switch Resilience: Fire-and-Forget Apply

## Problem

When the agent applies a generation that changes its own systemd service,
`switch-to-configuration` stops the agent mid-activation. The child process
is in the agent's cgroup and dies with it. The activation never completes,
the system profile never updates, and the agent loops on restart.

This is the root cause of agents going dead after self-affecting deploys.
Lock contention detection and `StartLimitIntervalSec=0` (already shipped)
handle adjacent problems but not this one.

## Solution

Replace the synchronous apply (spawn switch-to-configuration, wait for exit
code) with a fire-and-forget pattern: spawn the switch in a detached
transient systemd service, poll `/run/current-system` until it matches the
desired generation, then verify health and report.

The same pattern applies to rollback. On poll timeout, the agent fires a
rollback switch targeting the previous generation and polls again.

## Deploy Cycle (Revised)

```
check desired → fetch closure →
  report "applying" to CP →
  fire switch (systemd-run --unit=nixfleet-switch) →
  poll /run/current-system (2s interval, 5min timeout) →

  Agent may die here (self-switch kills it).
  On restart: startup check runs the same poll logic.

  current == desired?
    yes → run health checks → report success/failure
    no (timeout) →
      check nixfleet-switch exit status →
      if failed: retry fire+poll (up to MAX_APPLY_RETRIES) →
      if retries exhausted:
        resolve previous generation →
        fire rollback switch → poll for previous gen →
          matched → report "rolled back: reason"
          timeout → report "rollback failed"
```

## Components

### `fire_switch(store_path: &str) -> Result<()>`

In `agent/src/nix.rs`. Validates the store path, then spawns:

```
systemd-run --unit=nixfleet-switch -- <store_path>/bin/switch-to-configuration switch
```

Returns immediately after `systemd-run` exits (which happens as soon as
the transient unit is queued — the switch itself runs asynchronously in
`nixfleet-switch.service`). Errors only on spawn failure or if the
transient unit cannot be created (e.g., name conflict from a previous
run that hasn't been garbage-collected).

Does NOT capture stdout/stderr from switch-to-configuration — the switch
runs detached. Output goes to the journal under the transient unit name.

### `poll_current_generation(expected: &str, timeout: Duration, interval: Duration) -> Result<bool>`

In `agent/src/nix.rs`. Loops on `readlink /run/current-system`:

- Returns `Ok(true)` when current matches `expected`.
- Returns `Ok(false)` when `timeout` expires without a match.
- Returns `Err` on filesystem errors.

Uses `tokio::time::sleep(interval)` between checks. If the agent is
killed mid-poll (self-switch), the loop simply doesn't complete. On
restart, the startup check runs the same function.

### `check_switch_exit_status() -> Result<Option<bool>>`

In `agent/src/nix.rs`. Checks the transient unit's outcome:

```
systemctl show nixfleet-switch.service -p ActiveState,Result
```

- `ActiveState=inactive, Result=success` → `Ok(Some(true))`
- `ActiveState=inactive, Result=*` → `Ok(Some(false))` (switch failed)
- `ActiveState=active` → `Ok(None)` (still running)
- Unit not found → `Ok(None)` (never ran or already cleaned up)

Used after poll timeout to decide whether to retry or rollback.

### Changes to `run_deploy_cycle` in `agent/src/lib.rs`

The apply block becomes:

1. `send_report("applying")` — informs CP deploy is in progress.
2. Retry loop (up to `MAX_APPLY_RETRIES`):
   a. `fire_switch(&desired.hash)?`
   b. `poll_current_generation(&desired.hash, 5min, 2s)`
   c. If `true` → break (success)
   d. If `false` → `check_switch_exit_status()`:
      - Switch failed → log, retry from (a)
      - Switch still running or unknown → break (give up)
3. After loop: if current == desired → health checks → report success.
4. If current != desired (all retries failed) → rollback via same
   fire+poll pattern → report outcome.

### Startup Recovery

On startup, before entering the main poll loop, the agent runs a
lightweight recovery check:

1. Read current generation from `/run/current-system`.
2. Fetch desired generation from CP.
3. If current == desired: the agent was likely restarted mid-switch and
   the switch succeeded. Run health checks, report result.
4. If current != desired: normal startup — enter the poll loop, which
   will trigger a deploy cycle.

This is not new code — it's the same initial deploy cycle the agent
already runs on startup. The only difference is awareness that a match
on startup after a restart may indicate a completed self-switch, so
health checks should run before reporting.

Actually, this is exactly what the current startup flow already does:
run an initial deploy cycle, which checks desired, compares to current,
and if matched, reports "up-to-date". No change needed — the existing
startup behavior is the recovery path.

### Rollout Interaction

The "applying" report sent before firing the switch has
`current_generation = old` (the switch hasn't completed). The rollout
executor filters on generation match — it ignores reports where
`current_generation != expected`. The "applying" report is naturally
invisible to the rollout engine.

When the final report arrives (after poll + health), `current_generation`
matches the release entry and the rollout proceeds.

This is strictly better than the current behavior where a self-switch
kills the agent and it never reports back, stalling the rollout.

### Lock Contention

With fire-and-forget, the agent cannot detect lock contention from
switch-to-configuration's stderr (the process is detached). If the
switch fails due to lock contention:

1. The poll times out (current != desired after 5 min).
2. `check_switch_exit_status()` reports the unit failed.
3. The retry loop fires a new switch attempt.

This is slower than the current immediate stderr-based retry (5s vs
5min), but lock contention during agent-driven deploys is rare — the
common lock case is concurrent `nh os switch`, which operators should
avoid during rollouts.

The retry loop wraps the entire fire+poll cycle, preserving retry
semantics from the current design.

### Obsoleted code

`ApplyOutcome` enum, `is_lock_contention()`, and `apply_generation()`
from the current PR are replaced by `fire_switch` + `poll_current_generation`
+ `check_switch_exit_status`. The stderr-based lock detection is no longer
possible (detached process). Lock contention is detected indirectly via
poll timeout + exit status, and retried at the cycle level.

`apply_with_retry` in `lib.rs` adapts to wrap the fire+poll cycle instead
of wrapping `apply_generation`.

## Files Touched

- `agent/src/nix.rs` — `fire_switch`, `poll_current_generation`,
  `check_switch_exit_status` replace `apply_generation`
- `agent/src/lib.rs` — `run_deploy_cycle` uses fire+poll+retry pattern,
  `apply_with_retry` adapts to the new functions
- `agent/src/nix.rs` — `rollback()` uses `fire_switch` + `poll_current_generation`

## Testing

**Unit tests (tokio::time::pause):**
- `poll_current_generation`: mock `/run/current-system` via a temp symlink,
  verify returns `true` when updated, `false` on timeout.
- `check_switch_exit_status`: mock systemctl output, verify parsing.
- Retry loop: mock fire+poll outcomes, verify retry count and rollback trigger.

**Integration test:**
- Full fire+poll cycle with a real `systemd-run` on a test store path
  (won't actually switch, but verifies the plumbing).

**Manual validation (the real test):**
- Deploy a self-affecting generation on krach via rollout.
- Agent fires switch → gets killed → switch completes → agent restarts →
  polls → health → reports success → rollout proceeds.

## Out of Scope

- Concurrent switch detection (if `nixfleet-switch.service` already
  exists). For now, `systemd-run` rejects the duplicate and the agent
  treats it as a spawn failure.
- Progress reporting during the poll loop.
- Async rollback retry (if rollback times out, the agent stops).
- Reducing the 5-min poll timeout for lock contention cases.
