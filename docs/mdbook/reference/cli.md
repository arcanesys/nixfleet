# CLI

Flat reference for all `nixfleet` CLI commands and flags.

## Global options

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--control-plane-url` | `NIXFLEET_CP_URL` | `http://localhost:8080` | Control plane URL |
| `--api-key` | `NIXFLEET_API_KEY` | `""` | API key for control plane authentication |

Logging is controlled via `RUST_LOG` (default: `nixfleet=info`).

---

## deploy

Deploy config to fleet hosts.

```sh
nixfleet deploy [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--hosts <PATTERN>` | string | `*` | Host glob pattern (SSH mode only) |
| `--dry-run` | bool | `false` | Build closures and show plan, do not push |
| `--ssh` | bool | `false` | SSH fallback mode: copy closures and switch via SSH |
| `--flake <REF>` | string | `.` | Flake reference |
| `--tag <TAG>` | string (repeatable) | -- | Target machines by tag (rollout mode) |
| `--strategy <STRATEGY>` | string | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size <SIZES>` | string (comma-separated) | -- | Batch sizes (e.g., `1,25%,100%`) |
| `--failure-threshold <N>` | string | `1` | Max failures before pausing/reverting |
| `--on-failure <ACTION>` | string | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | u64 | `300` | Seconds to wait for health reports per batch |
| `--wait` | bool | `false` | Stream rollout progress to stdout |
| `--generation <PATH>` | string | -- | Store path hash (skips nix build, required for rollout mode) |

**Modes:**

- **SSH mode** (`--ssh`): Builds locally, copies closures via SSH, runs `switch-to-configuration`.
- **Rollout mode** (`--tag` + `--generation`): Creates a rollout on the control plane with the specified strategy.
- **Default mode**: Builds and deploys via the control plane.

---

## status

Show fleet status from the control plane.

```sh
nixfleet status [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | bool | `false` | Output as JSON |

---

## rollback

Rollback a host to a previous generation.

```sh
nixfleet rollback --host <HOST> [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--host <HOST>` | string | -- (required) | Target host name |
| `--generation <PATH>` | string | -- | Store path to roll back to (default: previous) |
| `--ssh` | bool | `false` | SSH fallback mode |

Without `--generation` in SSH mode, queries the target for `system-1-link`. Without `--generation` and without `--ssh`, the command errors (CP does not track generation history yet).

---

## host add

Scaffold a new host.

```sh
nixfleet host add --hostname <NAME> [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--hostname <NAME>` | string | -- (required) | Host name for the new machine |
| `--org <ORG>` | string | `my-org` | Organization name |
| `--role <ROLE>` | string | `workstation` | Host role (`workstation`, `server`, `edge`, `kiosk`) |
| `--platform <PLATFORM>` | string | `x86_64-linux` | Target platform |
| `--target <SSH>` | string | -- | SSH target to fetch hardware config (e.g., `root@192.168.1.42`) |

---

## host provision

Provision a host via nixos-anywhere.

```sh
nixfleet host provision --hostname <NAME> --target <SSH> [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--hostname <NAME>` | string | -- (required) | Host name (must exist in flake) |
| `--target <SSH>` | string | -- (required) | SSH target (e.g., `root@192.168.1.42`) |
| `--username <USER>` | string | `root` | Username for post-install verification |

---

## rollout list

List rollouts.

```sh
nixfleet rollout list [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--status <STATUS>` | string | -- | Filter by status (e.g., `running`, `paused`, `completed`) |

---

## rollout status

Show rollout detail with batch breakdown.

```sh
nixfleet rollout status <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Rollout ID |

---

## rollout resume

Resume a paused rollout.

```sh
nixfleet rollout resume <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Rollout ID |

---

## rollout cancel

Cancel a rollout.

```sh
nixfleet rollout cancel <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Rollout ID |

---

## machines list

List machines.

```sh
nixfleet machines list [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--tag <TAG>` | string | -- | Filter by tag |

---

## machines tag

Add tags to a machine.

```sh
nixfleet machines tag <ID> <TAGS...>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |
| `<TAGS...>` | string (one or more) | Tags to add |

---

## machines untag

Remove a tag from a machine.

```sh
nixfleet machines untag <ID> <TAG>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |
| `<TAG>` | string | Tag to remove |
