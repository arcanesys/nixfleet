# CLI

Flat reference for all `nixfleet` CLI commands and flags.

## Global options

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--control-plane-url` | `NIXFLEET_CP_URL` | `http://localhost:8080` | Control plane URL |
| `--api-key` | `NIXFLEET_API_KEY` | `""` | API key for control plane authentication |
| `--client-cert` | `NIXFLEET_CLIENT_CERT` | `""` | Client certificate for mTLS authentication |
| `--client-key` | `NIXFLEET_CLIENT_KEY` | `""` | Client key for mTLS authentication |
| `--ca-cert` | `NIXFLEET_CA_CERT` | `""` | CA certificate for TLS verification (uses system trust store if omitted) |

Logging is controlled via `RUST_LOG` (default: `nixfleet=info`).

**mTLS example:**

```sh
export NIXFLEET_CP_URL=https://cp-01:8080
export NIXFLEET_CA_CERT=/etc/nixfleet/fleet-ca.pem
export NIXFLEET_CLIENT_CERT=/run/agenix/cp-cert
export NIXFLEET_CLIENT_KEY=/run/agenix/cp-key
export NIXFLEET_API_KEY=nfk-...
nixfleet machines list  # no flags needed
```

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
| `--target <SSH>` | string | -- | SSH target override (e.g., `root@192.168.1.10`). Only valid with `--ssh` and a single host. |
| `--flake <REF>` | string | `.` | Flake reference |
| `--tag <TAG>` | string (repeatable) | -- | Target machines by tag (rollout mode) |
| `--strategy <STRATEGY>` | string | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size <SIZES>` | string (comma-separated) | -- | Batch sizes (e.g., `1,25%,100%`) |
| `--failure-threshold <N>` | string | `1` | Max failures before pausing/reverting |
| `--on-failure <ACTION>` | string | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | u64 | `300` | Seconds to wait for health reports per batch |
| `--wait` | bool | `false` | Stream rollout progress to stdout |
| `--generation <PATH>` | string | -- | Store path hash (skips nix build, required for rollout mode) |
| `--policy <NAME>` | string | -- | Use a named rollout policy (policy values serve as defaults; explicit flags override) |
| `--cache-url <URL>` | string | -- | Binary cache URL for agents to fetch closures from (e.g., `http://cache:8081`) |
| `--schedule-at <ISO8601>` | string | -- | Schedule the rollout for a future time (e.g., `2026-04-06T03:00:00Z`) |

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

## policy create

Create a named rollout policy.

```sh
nixfleet policy create --name <NAME> [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | -- (required) | Policy name (unique) |
| `--strategy <STRATEGY>` | string | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size <SIZES>` | string (comma-separated) | `100%` | Batch sizes (e.g., `1,25%,100%`) |
| `--failure-threshold <N>` | string | `1` | Max failures before pausing/reverting |
| `--on-failure <ACTION>` | string | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | u64 | `300` | Seconds to wait for health reports per batch |

---

## policy list

List all rollout policies.

```sh
nixfleet policy list
```

---

## policy get

Show detail for a named policy.

```sh
nixfleet policy get <NAME>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<NAME>` | string | Policy name |

---

## policy update

Update an existing policy. All flags replace the current values.

```sh
nixfleet policy update <NAME> [FLAGS]
```

| Argument/Flag | Type | Default | Description |
|---------------|------|---------|-------------|
| `<NAME>` | string | -- (required) | Policy name |
| `--strategy <STRATEGY>` | string | `all-at-once` | Rollout strategy |
| `--batch-size <SIZES>` | string (comma-separated) | `100%` | Batch sizes |
| `--failure-threshold <N>` | string | `1` | Max failures before pausing/reverting |
| `--on-failure <ACTION>` | string | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | u64 | `300` | Seconds to wait for health reports per batch |

---

## policy delete

Delete a policy (admin only).

```sh
nixfleet policy delete <NAME>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<NAME>` | string | Policy name |

---

## schedule list

List scheduled rollouts.

```sh
nixfleet schedule list [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--status <STATUS>` | string | -- | Filter by status: `pending`, `triggered`, `cancelled` |

---

## schedule cancel

Cancel a scheduled rollout.

```sh
nixfleet schedule cancel <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Schedule ID |

---

## bootstrap

Create the first admin API key. Only works when no keys exist in the control plane.

```sh
nixfleet bootstrap [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | `admin` | Name for the admin key |
| `--json` | bool | `false` | Output full JSON instead of human-friendly format |

**Output:** Human-friendly info to stderr, raw key to stdout. Scriptable:

```sh
API_KEY=$(nixfleet bootstrap)
```

Returns exit code 1 with an error message if keys already exist (409).

**Note:** No `--api-key` needed (chicken-and-egg). mTLS is still required when the CP has `--client-ca` set.

---

## machines register

Register a machine with the control plane (admin endpoint).

```sh
nixfleet machines register <ID> [FLAGS]
```

| Argument/Flag | Type | Description |
|---------------|------|-------------|
| `<ID>` | string | Machine ID |
| `--tag <TAG>` | string (repeatable) | Initial tags |

Agents auto-register on first health report, so manual registration is optional. Use this to pre-register machines before they come online.

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
