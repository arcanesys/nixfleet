# Rollback

Four mechanisms exist for rolling back, from fully automatic to fully manual.

## 1. Automatic (agent health checks)

When the agent applies a new generation, it runs the configured health checks (systemd units, HTTP endpoints, custom commands). If any check fails, the agent automatically:

1. Rolls back to the previous generation (`switch-to-configuration switch`)
2. Reports the failure to the control plane with `success: false`
3. Includes the rollback reason in the report message

No operator action required. During a rollout, this failure report triggers the rollout's health gate, which may pause or revert the entire rollout depending on `--on-failure` settings.

## 2. Rollout-level revert (`on_failure = revert`)

When a rollout is created with `--on-failure revert` and a later batch fails, the control plane reads each completed batch's `previous_generations` map (captured at batch start) and sets each machine's desired generation back to the store path it was running BEFORE the rollout started. This is per-machine - each host reverts to its own previous state, not a single shared generation. The rollout status becomes `failed` and agents pull the revert on their next poll (within ~5s due to `poll_hint`).

This is the correct rollback mechanism for heterogeneous fleets where each machine has a unique closure.

## 3. Manual via CLI (SSH mode)

`nixfleet rollback` is an SSH-only operation - it switches a single machine to a previous generation directly over SSH, bypassing the control plane.

```sh
# Rollback to the previous generation (reads from system-1-link on the target)
nixfleet rollback --host web-01 --ssh

# Rollback to a specific store path
nixfleet rollback --host web-01 --ssh --generation /nix/store/abc123-nixos-system
```

This runs `switch-to-configuration switch` on the target via SSH. Useful when the control plane is unreachable or during bootstrap before the agent is running.

For CP-driven rollback of a bad deploy discovered after health checks pass, deploy an older release:

```sh
git checkout <old-commit>
nixfleet release create --push-to ssh://root@cache
git checkout -
nixfleet deploy --release <old-id> --tags prod --wait
```

## 4. Manual via NixOS

Standard NixOS rollback mechanisms work regardless of NixFleet.

### Command-line rollback

```sh
# On the target machine
sudo nixos-rebuild switch --rollback

# Or switch to a specific generation
sudo nix-env -p /nix/var/nix/profiles/system --switch-generation 42
sudo /nix/var/nix/profiles/system/bin/switch-to-configuration switch
```

### Boot menu

systemd-boot lists previous generations at boot. Select an older entry to boot into a previous configuration. This is the last resort when SSH access is unavailable or the current generation fails to boot.

## When to use which

| Scenario | Mechanism |
|----------|-----------|
| Deployment health check fails | Automatic (agent rolls back per-machine) |
| Mid-rollout batch failure with `--on-failure revert` | Automatic (CP reverts completed batches from per-machine `previous_generations`) |
| Bad deploy discovered after health checks pass | Create a release pointing at the old closures, `nixfleet deploy --release <old>` |
| Control plane is down | SSH rollback (`nixfleet rollback --host <h> --ssh`) or NixOS boot menu |
| Machine won't boot | Boot menu (select previous generation) |
| Rollout affecting multiple machines | `nixfleet rollout cancel` + individual rollbacks if needed |
