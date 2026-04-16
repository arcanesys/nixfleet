# CLI

Flat reference for all `nixfleet` CLI commands and flags.

## Global options

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--control-plane-url` | `NIXFLEET_CONTROL_PLANE_URL` | `http://localhost:8080` | Control plane URL |
| `--api-key` | `NIXFLEET_API_KEY` | `""` | API key for control plane authentication |
| `--client-cert` | `NIXFLEET_CLIENT_CERT` | `""` | Client certificate for mTLS authentication |
| `--client-key` | `NIXFLEET_CLIENT_KEY` | `""` | Client key for mTLS authentication |
| `--ca-cert` | `NIXFLEET_CA_CERT` | `""` | CA certificate for TLS verification (uses system trust store if omitted) |
| `--json` | — | `false` | Output structured JSON (on commands that produce tables/detail views) |
| `--config` | — | — | Path to `.nixfleet.toml` (default: walk up from cwd) |
| `-v`, `--verbose` | — | `0` | Verbosity: `-v` shows INFO milestones + subprocess rolling window + progress bar; `-vv` shows raw passthrough (debug) |

Logging is controlled via `RUST_LOG` (overrides `-v`/`--verbose` when set).

### Configuration sources

The CLI reads connection settings from four layers, in priority order (highest wins):

1. **CLI flags** (`--control-plane-url`, `--api-key`, …)
2. **Environment variables** (`NIXFLEET_*` shown above)
3. **`~/.config/nixfleet/credentials.toml`** — user-level API keys, keyed by CP URL (auto-saved by `nixfleet bootstrap`)
4. **`.nixfleet.toml`** — repo-level config, from `--config <path>` or discovered by walking up from cwd

This means the same CLI commands run with no flags from any fleet repo, inheriting the repo's connection settings and the user's bootstrapped credentials. See [`.nixfleet.toml` format](#nixfleet-toml-format) below.

**mTLS example (with config file):**

```sh
# One-time setup (creates .nixfleet.toml)
nixfleet init \
  --control-plane-url https://cp-01:8080 \
  --ca-cert modules/_config/fleet-ca.pem \
  --client-cert '/run/agenix/agent-${HOSTNAME}-cert' \
  --client-key '/run/agenix/agent-${HOSTNAME}-key' \
  --cache-url http://cache:5000 \
  --push-to ssh://root@cache

# Bootstrap first admin key (auto-saves to ~/.config/nixfleet/credentials.toml)
nixfleet bootstrap

# Subsequent commands: no flags needed
nixfleet machines list
nixfleet release create
nixfleet deploy --release rel-abc123 --hosts 'web-*' --wait
```

---

## deploy

Deploy config to fleet hosts.

```sh
nixfleet deploy [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--release <ID>` | string | -- | Deploy an existing release (required for rollout mode unless using `--push-to` / `--copy`) |
| `--push-to <URL>` | string | -- | Build all hosts, push to a Nix binary cache URL, and register a release implicitly (e.g., `ssh://root@cache`, `s3://bucket`) |
| `--hook` | bool | `false` | Use hook mode: push via `[cache.hook] push-cmd` instead of `nix copy`. Requires `[cache.hook]` in `.nixfleet.toml` or `--hook-push-cmd` |
| `--hook-push-cmd <CMD>` | string | -- | Override hook push command (`{}` = store path). Requires `--hook` |
| `--hook-url <URL>` | string | -- | Override hook cache URL for agents to pull from. Requires `--hook` |
| `--copy` | bool | `false` | Build all hosts, push to each target via `nix-copy-closure` (no binary cache needed), and register a release implicitly |
| `--hosts <PATTERN>` | string (comma-separated or repeatable) | `*` | Host glob patterns. In SSH mode: hosts to deploy. In rollout mode: target machines directly (alternative to `--tags`) |
| `--tags <TAG>` | string (comma-separated or repeatable) | -- | Target machines by tag (rollout mode) |
| `--dry-run` | bool | `false` | Build closures and show plan, do not push or register |
| `--ssh` | bool | `false` | SSH fallback mode: build locally, copy via SSH, run `switch-to-configuration` (no CP needed) |
| `--target <SSH>` | string | -- | SSH target override (e.g., `root@192.168.1.10`). Only valid with `--ssh` and a single host. |
| `--flake <REF>` | string | `.` | Flake reference |
| `--strategy <STRATEGY>` | string | `all-at-once` | Rollout strategy: `canary`, `staged`, `all-at-once` |
| `--batch-size <SIZES>` | string (comma-separated) | -- | Batch sizes (e.g., `1,25%,100%`) |
| `--failure-threshold <N>` | string | `0` | Max unhealthy machines per batch before pausing/reverting. Accepts absolute count or percentage (e.g. `30%`) |
| `--on-failure <ACTION>` | string | `pause` | Action on failure: `pause` or `revert` |
| `--health-timeout <SECS>` | u64 | `300` | Seconds to wait for health reports per batch |
| `--wait` | bool | `false` | Stream rollout progress to stdout |
| `--cache-url <URL>` | string | -- | Binary cache URL for agents to fetch closures from (overrides the release's cache_url) |

**Modes:**

- **SSH mode** (`--ssh`): Builds locally, copies closures via SSH, activates on target. No control plane required. Platform-aware: NixOS hosts use `switch-to-configuration switch`, Darwin hosts use `nix-env --set` + `activate` (auto-detected from the host's platform).

> **Note:** `--ssh` deploys directly via `nix-copy-closure` and activation,
> bypassing the control plane entirely. Lifecycle state is not checked — a machine in
> `maintenance` will still receive the deploy. Use `--ssh` as an emergency escape hatch
> when the CP is unavailable, not as a routine deployment method.
- **Rollout mode** (requires a release): Creates a rollout on the control plane with the specified strategy. Specify an existing release with `--release <ID>`, or use `--push-to <url>` / `--hook` / `--copy` to build + push + register implicitly in one command.
- **Hook mode** (`--hook`): Uses `[cache.hook] push-cmd` from `.nixfleet.toml` to push closures (e.g., `attic push mycache {}`). Overrides `--push-to` and uses `[cache.hook] url` as the cache URL for agents. Flags `--hook-push-cmd` and `--hook-url` override the config values.
- **Targeting:** Use `--tags <TAG>` or `--hosts <pattern>` to select machines. Both are intersected with the release's host list (machines not in the release are skipped with a warning).

---

## init

Create a `.nixfleet.toml` config file in the current directory. Run this once per fleet repo to set the connection and deploy defaults.

```sh
nixfleet init [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--control-plane-url <URL>` | string | -- (required) | Control plane URL |
| `--ca-cert <PATH>` | string | -- | CA certificate path (relative to config file or absolute) |
| `--client-cert <PATH>` | string | -- | Client certificate path (supports `${HOSTNAME}` expansion) |
| `--client-key <PATH>` | string | -- | Client key path (supports `${HOSTNAME}` expansion) |
| `--cache-url <URL>` | string | -- | Default binary cache URL for agents |
| `--push-to <URL>` | string | -- | Default push destination for `release create` |
| `--hook-url <URL>` | string | -- | Hook mode cache URL (e.g., `http://cache:8081/mycache` for Attic) |
| `--hook-push-cmd <CMD>` | string | -- | Hook mode push command (`{}` = store path, e.g., `attic push mycache {}`) |

After `init`, run `nixfleet bootstrap` to create and auto-save the first admin API key.

---

## release create

Build host closures, distribute them, and register a release manifest in the control plane. A release is an immutable mapping of hostnames to built store paths that subsequent rollouts can target.

```sh
nixfleet release create [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--flake <REF>` | string | `.` | Flake reference |
| `--hosts <PATTERN>` | string | `*` | Host glob pattern or comma-separated list |
| `--push-to <URL>` | string | -- | Push closures to this Nix cache URL via `nix copy --to` (e.g., `ssh://root@cache`, `s3://bucket`) |
| `--hook` | bool | `false` | Use hook mode: push via `[cache.hook] push-cmd` instead of `nix copy` |
| `--hook-push-cmd <CMD>` | string | -- | Override hook push command (`{}` = store path). Requires `--hook` |
| `--hook-url <URL>` | string | -- | Override hook cache URL. Requires `--hook` |
| `--copy` | bool | `false` | Push closures directly to each target host via `nix-copy-closure` (no binary cache) |
| `--cache-url <URL>` | string | -- | Override the cache URL recorded in the release (defaults to `--push-to` URL, or config file) |
| `--eval-only` | bool | `false` | Evaluate `config.system.build.toplevel.outPath` without building. Assumes closures are already in the cache (e.g., CI-built). Useful for cross-platform fleets where the operator cannot build non-native closures locally. |
| `--dry-run` | bool | `false` | Build and show the manifest without registering |

Output prints the release ID, host count, and per-host store paths. Use the ID with `nixfleet deploy --release <ID>`.

---

## release list

List recent releases.

```sh
nixfleet release list [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--limit <N>` | u32 | `20` | Number of releases to show (newest first) |
| `--host <HOSTNAME>` | string | -- | Filter releases to those containing entries for this hostname |

---

## release show

Show a release's full metadata and per-host entries.

```sh
nixfleet release show <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Release ID |

---

## release diff

Diff two releases: added hosts, removed hosts, changed store paths, unchanged.

```sh
nixfleet release diff <ID_A> <ID_B>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID_A>` | string | First release ID |
| `<ID_B>` | string | Second release ID |

---

## release delete

Delete a release. Fails with exit code 1 if the release is still referenced by a rollout — the control plane returns 409 in that case to prevent breaking rollout history.

```sh
nixfleet release delete <RELEASE_ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<RELEASE_ID>` | string | ID of the release to delete |

Exit codes:
- `0` — release deleted (CP returned 204)
- `1` — release still referenced by a rollout (CP returned 409), release not found (CP returned 404), or another non-2xx status

---

## status

Show fleet status from the control plane.

```sh
nixfleet status
```

Outputs a table of all machines. Pass `--json` (global flag) for structured JSON output.

---

## rollback

Rollback a single machine to a previous generation via SSH. This is an SSH-only operation — it runs `switch-to-configuration switch` directly on the target, bypassing the control plane.

```sh
nixfleet rollback --host <HOST> --ssh [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--host <HOST>` | string | -- (required) | Target host name |
| `--ssh` | bool | `false` | **Required.** SSH mode |
| `--generation <PATH>` | string | -- | Store path to roll back to (default: previous generation from `system-1-link`) |
| `--target` | string | — | SSH target override (e.g. `root@192.168.1.10`) |
| `--darwin` | bool | `false` | Target is a Darwin (macOS) host — uses `activate` instead of `switch-to-configuration` |

Running without `--ssh` exits with an error. For CP-driven rollback, use `--on-failure revert` on rollouts, or deploy an older release.

**Darwin rollback:** Use `--darwin` for macOS hosts. This runs `nix-env --set` + `activate` instead of `switch-to-configuration`:

```sh
nixfleet rollback --host aether --ssh --darwin
```

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

## bootstrap

Create the first admin API key. Only works when no keys exist in the control plane.

```sh
nixfleet bootstrap [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | `admin` | Name for the admin key |
| `--save-key <KEY>` | string | -- | Save an existing API key without calling the CP (for setting up additional machines) |

**Output:** Human-friendly info to stderr, raw key to stdout. Scriptable:

```sh
API_KEY=$(nixfleet bootstrap)
```

Returns exit code 1 with an error message if keys already exist (409).

**Note:** No `--api-key` needed (chicken-and-egg). mTLS is still required when the CP has `--client-ca` set.

**Multi-machine setup:** Bootstrap once on your primary machine, then use `--save-key` on additional machines to share the same API key without re-bootstrapping:

```sh
# On the primary machine:
nixfleet bootstrap

# On additional machines (same fleet):
nixfleet bootstrap --save-key nfk-abc123...
```

---

## machines register

Register a machine with the control plane (admin endpoint).

```sh
nixfleet machines register <ID> [FLAGS]
```

| Argument/Flag | Type | Description |
|---------------|------|-------------|
| `<ID>` | string | Machine ID |
| `--tags <TAG>` | string (comma-separated or repeatable) | Initial tags |

Agents auto-register on first health report, so manual registration is optional. Use this to pre-register machines before they come online.

---

## machines list

List machines.

```sh
nixfleet machines list [FLAGS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--tags <TAG>` | string (comma-separated or repeatable) | -- | Filter by tags (machines matching any listed tag are shown) |

---

## machines set-lifecycle

Change a machine's lifecycle state.

```sh
nixfleet machines set-lifecycle <ID> <STATE>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |
| `<STATE>` | string | Lifecycle state: `active`, `pending`, `provisioning`, `maintenance`, `decommissioned` |

Only `active` machines participate in rollouts. Machines in `maintenance` or
`decommissioned` state are excluded even when explicitly targeted by hostname.
Use `maintenance` to temporarily remove a machine from fleet operations without
deregistering it.

---

## machines clear-desired

Clear a machine's stale desired generation. Use this when an agent is stuck polling for a generation that will never be fulfilled (e.g., after a cancelled rollout).

```sh
nixfleet machines clear-desired <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |

Exit codes:
- `0` — desired generation cleared (CP returned 204)
- `1` — machine not found (CP returned 404), or another non-2xx status

---

## machines notify-deploy

Notify the control plane of an out-of-band deploy (e.g. SSH). Sets the machine's desired generation to the deployed store path so `nixfleet status` shows the machine in sync once the agent confirms.

Called automatically by `deploy --ssh` after a successful switch. Also available manually for other out-of-band deploy workflows.

```sh
nixfleet machines notify-deploy <ID> <STORE_PATH>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |
| `<STORE_PATH>` | string | Store path that was deployed |

Requires `deploy` or `admin` role.

---

## rollout delete

Delete a terminal rollout (completed, cancelled, or failed). The control plane rejects deletion of active rollouts with 409.

```sh
nixfleet rollout delete <ID>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Rollout ID |

Exit codes:
- `0` — rollout deleted (CP returned 204)
- `1` — rollout is still active (CP returned 409), rollout not found (CP returned 404), or another non-2xx status

---

## Operation logs

All CLI operations (deploy, release create, rollout commands) write persistent logs to:

```
~/.local/state/nixfleet/logs/
```

Each operation creates a JSONL file with timestamped entries covering subprocess invocations (command, stdout, stderr, exit code), tracing events, and host context. Logs are written regardless of verbosity level.

---

## `.nixfleet.toml` format <a id="nixfleet-toml-format"></a>

Committed to the fleet repo root. Discovered by walking up from the CLI's current working directory. All fields optional — CLI flags and environment variables always override.

```toml
[control-plane]
url = "https://cp.example.com:8080"
ca-cert = "modules/_config/fleet-ca.pem"    # relative to config file location

[tls]
client-cert = "/run/agenix/agent-${HOSTNAME}-cert"
client-key = "/run/agenix/agent-${HOSTNAME}-key"

[cache]
url = "http://cache.example.com:5000"          # default --cache-url for rollouts
push-to = "ssh://root@cache.example.com"       # default --push-to for release create

[cache.hook]                                    # used when --hook is passed
url = "http://cache.example.com:8081/mycache"   # overrides cache.url for the release
push-cmd = "attic push mycache {}"              # {} is replaced with the store path

[deploy]
strategy = "staged"             # default rollout strategy
health-timeout = 300            # default health timeout in seconds
failure-threshold = "0"
on-failure = "pause"
```

**Environment variable expansion:** values support `${VAR}` expansion. `${HOSTNAME}` and `${HOST}` fall back to the `gethostname()` syscall if not set in the environment (so they work from zsh where `$HOST` is a shell builtin, not exported). This lets the same `.nixfleet.toml` work across every fleet host when agent cert paths follow a per-hostname convention.

**Relative paths** (like `ca-cert = "modules/_config/fleet-ca.pem"`) are resolved relative to the `.nixfleet.toml` location, not the CLI's working directory.

## `~/.config/nixfleet/credentials.toml` format

User-level, mode 600, not checked into any repo. Written automatically by `nixfleet bootstrap` and keyed by CP URL to support multiple clusters.

```toml
["https://cp.example.com:8080"]
api-key = "nfk-73c713cc..."

["https://cp-staging.example.com:8080"]
api-key = "nfk-abc..."
```

On impermanent NixOS hosts, add `.config/nixfleet` to home-manager persistence so the credentials file survives reboots.
