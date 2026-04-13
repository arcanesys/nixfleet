# ADR-012: Fire-and-Forget Apply for Self-Switch Resilience

**Status:** Accepted
**Date:** 2026-04-13

## Context

The agent applies NixOS generations by running `switch-to-configuration switch`. When the new generation changes the agent's own systemd service, `switch-to-configuration` stops and restarts the agent. In the original synchronous design, `switch-to-configuration` was a child process in the agent's cgroup. When systemd stopped the agent, it killed all processes in the cgroup — including `switch-to-configuration` itself. The activation never completed, the system profile never updated, and the agent looped on restart.

Attempted mitigations:
- `systemd-run --scope`: the scope is created under the agent's service cgroup and dies with it.
- `systemd-run --pipe --wait`: the agent dies before reading the result, leaving the pipe orphaned.

## Decision

Replace the synchronous apply (spawn, wait for exit code) with a fire-and-forget pattern:

1. The agent spawns `switch-to-configuration` in a **detached transient systemd service** via `systemd-run --unit=nixfleet-switch`. This creates a fully independent unit, not in the agent's cgroup.
2. The agent **does not wait** for the switch to complete. It polls `/run/current-system` every 2 seconds until the symlink matches the desired generation (300s timeout).
3. If the agent gets killed mid-poll (self-switch), `nixfleet-switch.service` continues independently. When the activation completes, systemd starts the new agent. The startup deploy cycle detects current == desired and reports success.
4. If the poll times out, the agent checks `systemctl show nixfleet-switch.service` for the exit status. Failed → retry (up to 3 times). Inconclusive → give up and rollback.
5. Rollback uses the same fire-and-forget mechanism (fire switch with previous generation, poll for match).

The agent sends a `"applying"` report to the control plane before firing the switch. This report has `current_generation = old` (the switch hasn't completed), so the rollout engine's generation-match filter naturally ignores it. The final report (after poll + health checks) carries the correct generation.

## Consequences

**Positive:**
- The agent survives self-switch. The activation always completes because it runs in an independent systemd unit.
- Rollouts no longer stall on machines that receive agent-affecting generations.
- The startup deploy cycle is the recovery path — no special restart-detection logic needed.
- Lock contention from concurrent rebuilds is handled indirectly: the switch fails in the background, poll times out, the agent retries.

**Negative:**
- The agent cannot read `switch-to-configuration` stderr (the process is detached). Lock contention and other failures are detected indirectly via poll timeout + exit status, which is slower than direct stderr parsing (~300s vs ~5s).
- A stale `nixfleet-switch.service` from a previous run could block `systemd-run` from creating a new one. The agent treats this as a spawn failure.
- The `"applying"` report briefly shows the machine in an intermediate state in the control plane (old generation + success=true + message="applying"). The rollout engine ignores it, but `nixfleet status` may show it during the ~2-30s switch window.

**Trade-offs accepted:**
- Slower lock contention detection (rare during agent-driven deploys — the common case is the agent being the only process switching).
- Fire-and-forget means the agent trusts systemd to manage the transient unit lifecycle. If systemd itself has issues, the agent has no recourse.
