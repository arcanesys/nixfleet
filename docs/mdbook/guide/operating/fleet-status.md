# Fleet Status

Day-2 operations for monitoring your fleet through the CLI and control plane.

## Fleet overview

```sh
nixfleet status
```

Shows a summary of all machines known to the control plane: hostname, current generation (from the agent's most recent report), desired generation (from the active rollout's release entry, if any), lifecycle state, last report time, and tags.

For machine-readable output:

```sh
nixfleet status --json
```

## Listing machines

```sh
nixfleet machines list
```

Filter by tag:

```sh
nixfleet machines list --tags prod
nixfleet machines list --tags web
```

## Tags

Tags group machines for targeted deployments and filtering. They can be set in two places.

### Via NixOS configuration

Declare tags in the agent service config. These are baked into the system closure and reported on every poll:

```nix
services.nixfleet-agent = {
  enable = true;
  controlPlaneUrl = "https://fleet.example.com";
  tags = ["prod" "web" "region-eu"];
};
```

Tags are stored in the control plane database. NixOS-configured tags (from `services.nixfleet-agent.tags`) are reported by the agent on every poll and synced to the control plane.

## Machine lifecycle

Every machine has a lifecycle state that determines how the control plane treats it.

| State | Description |
|-------|-------------|
| `pending` | Pre-registered, no agent report yet |
| `provisioning` | Install in progress |
| `active` | Agent reporting normally |
| `maintenance` | Manually paused |
| `decommissioned` | Removed from fleet |

Lifecycle is informational - rollouts target machines by tag or hostname regardless of lifecycle state. Use lifecycle to track operational status and filter with `nixfleet machines list`.

### Transitions

Not all transitions are valid. The control plane enforces these rules:

```
pending --> provisioning --> active
pending --> active                    (agent reports directly)
pending --> decommissioned            (never used)
provisioning --> pending              (reset)
active <--> maintenance              (pause/resume)
active --> decommissioned             (retire)
maintenance --> decommissioned        (retire while paused)
```

Invalid transitions (e.g., `decommissioned` to `active`, or `active` to `pending`) are rejected by the control plane.

### Changing lifecycle state

Use the control plane API directly:

```sh
curl -X PATCH "$NIXFLEET_CONTROL_PLANE_URL/api/v1/machines/web-01/lifecycle" \
  -H "Content-Type: application/json" \
  -d '{"lifecycle": "maintenance"}'
```

## When the control plane is unavailable

The CLI's `status` and `machines list` commands require a running control plane. If the CP is down:

- Agents continue running with their last-known generation
- Agents do not receive new deployments
- Use SSH for direct machine access (`ssh root@hostname`)
- Use standard NixOS tools for local inspection (`nixos-rebuild list-generations`, `systemctl status`)
