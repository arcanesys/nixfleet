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
| `-v`, `--verbose` | — | `0` | Increase verbosity (-v for info, -vv for debug). Default: warn. |

Logging is controlled via `RUST_LOG` (overrides `-v`/`--verbose` when set).

### Configuration sources

The CLI reads connection settings from four layers, in priority order (highest wins):

1. **CLI flags** (`--control-plane-url`, `--api-key`, …)
2. **Environment variables** (`NIXFLEET_*` shown above)
3. **`~/.config/nixfleet/credentials.toml`** — user-level API keys, keyed by CP URL (auto-saved by `nixfleet bootstrap`)
4. **`.nixfleet.toml`** — repo-level config, discovered by walking up from cwd

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
| `--push-hook <CMD>` | string | -- | Run a shell command after pushing each closure (escape hatch for Attic/Cachix). `{}` is replaced with the store path. Runs on the `--push-to` host when combined, otherwise locally |
| `--copy` | bool | `false` | Build all hosts, push to each target via `nix-copy-closure` (no binary cache needed), and register a release implicitly |
| `--hosts <PATTERN>` | string | `*` | Host glob pattern or comma-separated list. In SSH mode: hosts to deploy. In rollout mode: target machines directly (alternative to `--tag`) |
| `--tag <TAG>` | string (repeatable) | -- | Target machines by tag (rollout mode) |
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

- **SSH mode** (`--ssh`): Builds locally, copies closures via SSH, runs `switch-to-configuration`. No control plane required.
- **Rollout mode** (requires a release): Creates a rollout on the control plane with the specified strategy. Specify an existing release with `--release <ID>`, or use `--push-to <url>` / `--copy` to build + push + register implicitly in one command.
- **Targeting:** Use `--tag <TAG>` or `--hosts <pattern>` to select machines. Both are intersected with the release's host list (machines not in the release are skipped with a warning).

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
| `--push-hook <CMD>` | string | -- | Run this command after pushing each closure. `{}` is replaced with the store path. When combined with `--push-to`, runs on the remote host via SSH |
| `--copy` | bool | `false` | Push closures directly to each target host via `nix-copy-closure` (no binary cache) |
| `--cache-url <URL>` | string | -- | Override the cache URL recorded in the release (defaults to `--push-to` URL, or config file) |
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

Running without `--ssh` exits with an error. For CP-driven rollback, use `--on-failure revert` on rollouts, or deploy an older release.

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

## machines untag

Remove a tag from a machine.

```sh
nixfleet machines untag <ID> <TAG>
```

| Argument | Type | Description |
|----------|------|-------------|
| `<ID>` | string | Machine ID |
| `<TAG>` | string | Tag to remove |

---

## `.nixfleet.toml` format <a id="nixfleet-toml-format"></a>

Committed to the fleet repo root. Discovered by walking up from the CLI's current working directory. All fields optional — CLI flags and environment variables always override.

```toml
[control-plane]
url = "https://lab:8080"
ca-cert = "modules/_config/fleet-ca.pem"    # relative to config file location

[tls]
client-cert = "/run/agenix/agent-${HOSTNAME}-cert"
client-key = "/run/agenix/agent-${HOSTNAME}-key"

[cache]
url = "http://lab:5000"         # default --cache-url for rollouts
push-to = "ssh://root@lab"      # default --push-to for release create

[deploy]
strategy = "staged"             # default rollout strategy
health-timeout = 300            # default health timeout in seconds
failure-threshold = "1"
on-failure = "pause"
```

**Environment variable expansion:** values support `${VAR}` expansion. `${HOSTNAME}` and `${HOST}` fall back to the `gethostname()` syscall if not set in the environment (so they work from zsh where `$HOST` is a shell builtin, not exported). This lets the same `.nixfleet.toml` work across every fleet host when agent cert paths follow a per-hostname convention.

**Relative paths** (like `ca-cert = "modules/_config/fleet-ca.pem"`) are resolved relative to the `.nixfleet.toml` location, not the CLI's working directory.

## `~/.config/nixfleet/credentials.toml` format

User-level, mode 600, not checked into any repo. Written automatically by `nixfleet bootstrap` and keyed by CP URL to support multiple clusters.

```toml
["https://lab:8080"]
api-key = "nfk-73c713cc..."

["https://staging.cp.example.com:8080"]
api-key = "nfk-abc..."
```

On impermanent NixOS hosts, add `.config/nixfleet` to home-manager persistence so the credentials file survives reboots.
