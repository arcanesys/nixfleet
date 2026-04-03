# Rollback

Three mechanisms exist for rolling back a machine to a previous NixOS generation, from fully automatic to fully manual.

## 1. Automatic (agent health checks)

When the agent applies a new generation, it runs the configured health checks (systemd units, HTTP endpoints, custom commands). If any check fails, the agent automatically:

1. Rolls back to the previous generation (`switch-to-configuration switch`)
2. Reports the failure to the control plane with `success: false`
3. Includes the rollback reason in the report message

No operator action required. During a rollout, this failure report triggers the rollout's health gate, which may pause or revert the entire rollout depending on `--on-failure` settings.

## 2. Manual via CLI

### Via control plane

```sh
nixfleet rollback --host web-01 --generation /nix/store/abc123-nixos-system
```

This tells the control plane to set the desired generation for `web-01` to the specified store path. The agent picks up the change on its next poll and switches to it.

Without `--generation`, the CLI requires `--ssh` mode (the control plane does not track generation history yet).

### Via SSH

```sh
nixfleet rollback --host web-01 --ssh
```

Without `--generation`, this queries the target machine for its previous generation profile (`system-1-link`) and switches to it over SSH.

With an explicit generation:

```sh
nixfleet rollback --host web-01 --ssh --generation /nix/store/abc123-nixos-system
```

This runs `switch-to-configuration switch` directly on the target via SSH, bypassing the control plane entirely.

## 3. Manual via NixOS

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
| Deployment health check fails | Automatic (agent handles it) |
| Bad deploy discovered after health checks pass | CLI rollback (`--host` + `--generation`) |
| Control plane is down | SSH rollback or NixOS boot menu |
| Machine won't boot | Boot menu (select previous generation) |
| Rollout affecting multiple machines | `nixfleet rollout cancel` + individual rollbacks if needed |
