# NixFleet

Declarative NixOS fleet management framework. Nix modules + Rust agent/control-plane/CLI.

## Structure

```
modules/
├── _shared/lib/       # Framework API: mkHost, mkVmApps
├── _shared/           # hostSpec options, disk templates
├── core/              # Core NixOS/Darwin modules (_nixos.nix, _darwin.nix)
├── scopes/            # Scope modules (_base, _impermanence, _firewall, _secrets, _backup, _monitoring, nixfleet/_agent, nixfleet/_control-plane, nixfleet/_cache-server, nixfleet/_cache, nixfleet/_microvm-host)
├── tests/             # Eval tests, VM tests, integration tests
├── apps.nix           # Flake apps (validate, build-vm, start-vm, stop-vm, clean-vm, test-vm)
├── fleet.nix          # Framework test fleet
└── flake-module.nix   # Framework exports (lib.mkHost, nixosModules, diskoTemplates)
agent/                 # Rust: nixfleet-agent (sequential deploy cycle daemon — main.rs is a thin wrapper around lib.rs::run_loop)
control-plane/         # Rust: nixfleet-control-plane (Axum HTTP server, MtlsAcceptor for peer-cert injection, auth_cn middleware for CN validation)
cli/                   # Rust: nixfleet CLI (deploy, status, rollback, release, rollout, machines, host, init, bootstrap)
shared/                # Rust: nixfleet-types (shared data types)
examples/
├── client-fleet/      # Example: fleet consuming mkHost via flake-parts
├── standalone-host/   # Example: single machine in its own repo
└── batch-hosts/       # Example: 50 edge devices from a template
docs/
├── adr/               # Architecture Decision Records
└── mdbook/            # Technical reference + user guide (mdbook)
TODO.md                # External dependencies / future work
```

## Commands

```sh
# Dev shell
nix develop                        # dev shell (cargo, rustc, clippy, rustfmt)
nix fmt                            # format (alejandra + rustfmt + shfmt)

# Testing — ONE command for the whole suite
nix run .#validate -- --all        # full suite
nix run .#validate                 # fast (format + eval + hosts only)
nix run .#validate -- --rust       # + cargo test + clippy + package builds
nix run .#validate -- --vm         # + every vm-* check

# VM lifecycle
nix run .#build-vm -- -h web-02    # install VM (ISO + nixos-anywhere)
nix run .#start-vm -- -h web-02    # start VM as headless daemon
nix run .#stop-vm -- -h web-02     # stop VM daemon
nix run .#clean-vm -- -h web-02    # delete VM disk + state
nix run .#test-vm -- -h web-02     # end-to-end VM test cycle

# Deployment (standard NixOS tooling)
nixos-anywhere --flake .#hostname root@ip       # fresh install
sudo nixos-rebuild switch --flake .#hostname    # local rebuild
nixos-rebuild switch --flake .#hostname --target-host root@ip  # remote rebuild
darwin-rebuild switch --flake .#hostname        # macOS rebuild
```

VM apps reference: `docs/mdbook/reference/apps.md`

## Framework API

`nixfleet.lib.mkHost { hostName, platform, hostSpec?, modules?, stateVersion?, isVm? }` — returns `nixosSystem` or `darwinSystem`. `nixfleet.lib.mkVmApps` — VM helper apps for fleet repos.

Full parameter reference, injected modules, exports: `docs/mdbook/reference/mkhost-api.md`

## Framework Scopes

Scopes are plain NixOS/HM modules auto-included by mkHost. They self-activate via `lib.mkIf` on hostSpec flags.

**Automatic** (hostSpec-gated): `base` (`!isMinimal`), `impermanence` (`isImpermanent`), `firewall` (`!isMinimal`)

**Opt-in** (explicit enable): `secrets`, `backup`, `monitoring`, `nixfleet-agent`, `nixfleet-control-plane`, `nixfleet-cache-server`, `nixfleet-cache`, `nixfleet-microvm-host`

Fleet repos add opinionated scopes (dev tools, desktop environments, theming) as plain NixOS/HM modules.

Full scope table with activation conditions and details: `docs/mdbook/guide/defining-hosts/scopes.md`. Per-scope option reference: `docs/mdbook/reference/`.

## CLI

Commands: `init`, `bootstrap`, `status`, `deploy`, `rollback`, `release` (create/list/show/diff/delete), `rollout` (list/status/resume/cancel), `machines` (list/register/untag), `host` (add).

```bash
nixfleet init --control-plane-url https://lab:8080 --ca-cert fleet-ca.pem
nixfleet bootstrap                                                       # first admin API key
nixfleet deploy --push-to ssh://root@cache --tags web --strategy canary --wait
nixfleet deploy --hook --tags web --strategy canary --wait               # push via [cache.hook]
nixfleet deploy --hosts web-02 --ssh                                     # direct SSH (no CP)
nixfleet rollback --host web-02 --ssh                                    # SSH-only rollback
```

Config priority (highest wins): CLI flags → env vars → `~/.config/nixfleet/credentials.toml` → `.nixfleet.toml` (via `--config <path>` or cwd walk)

Full CLI reference with all flags, config format, and examples: `docs/mdbook/reference/cli.md`

### Rollout Executor & Generation Gating

The rollout executor verifies that each agent's `current_generation` matches the release entry before accepting health reports during batch evaluation. Mismatched generations are flagged as out-of-sync and paused. Details: `docs/mdbook/guide/deploying/rollouts.md`

## Control Plane API

### API Endpoints

#### Agent-facing (mTLS, no API key)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/machines/{id}/desired-generation` | Poll for desired state |
| POST | `/api/v1/machines/{id}/report` | Report deploy result + health |

#### Admin (API key required, role-gated)

Roles: `admin` (full access), `deploy` (create releases/rollouts), `readonly` (read-only).

| Method | Path | Min role | Description |
|--------|------|----------|-------------|
| GET | `/api/v1/machines` | readonly | List all machines |
| POST | `/api/v1/machines/{id}/register` | admin | Pre-register a machine |
| PATCH | `/api/v1/machines/{id}/lifecycle` | admin | Change machine lifecycle state |
| DELETE | `/api/v1/machines/{id}/tags/{tag}` | admin | Remove a tag from a machine |
| POST | `/api/v1/rollouts` | deploy | Create a rollout (requires `release_id`) |
| GET | `/api/v1/rollouts` | readonly | List rollouts |
| GET | `/api/v1/rollouts/{id}` | readonly | Get rollout detail (includes `events` timeline) |
| POST | `/api/v1/rollouts/{id}/resume` | deploy | Resume a paused rollout |
| POST | `/api/v1/rollouts/{id}/cancel` | deploy | Cancel a rollout |
| POST | `/api/v1/releases` | deploy | Create a release from a manifest |
| GET | `/api/v1/releases` | readonly | List releases (paginated, newest first) |
| GET | `/api/v1/releases/{id}` | readonly | Get release with entries |
| GET | `/api/v1/releases/{id}/diff/{other_id}` | readonly | Diff two releases |
| DELETE | `/api/v1/releases/{id}` | admin | Delete a release (409 if referenced by a rollout) |
| GET | `/api/v1/audit` | readonly | List audit events |
| GET | `/api/v1/audit/export` | readonly | Export audit events as CSV |

#### Bootstrap (no auth, only works when no keys exist)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/keys/bootstrap` | Create the first admin API key (409 if keys already exist) |

#### Unauthenticated

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check |
| GET | `/metrics` | Prometheus metrics |

### Agent tag sync

Agent tags (from `services.nixfleet-agent.tags`) are automatically synced to the CP on every health report. No manual tag management needed — tags are self-managing from NixOS config.

## Consuming the Framework

See `examples/` for standalone-host, batch-hosts, and client-fleet patterns. Quick start: `docs/mdbook/guide/getting-started/quick-start.md`

## Testing

```sh
nix run .#validate -- --all        # full suite: format + eval + hosts + VM + Rust + clippy + package builds
nix run .#validate                 # fast: format + eval + hosts only
nix run .#validate -- --rust       # + cargo test + clippy + rust package builds
nix run .#validate -- --vm         # + every vm-* check
```

Tiers: eval (instant), integration (mock consumer), VM framework (per-subsystem), VM fleet scenarios (multi-node), Rust unit + integration.

Full testing guide: `docs/mdbook/testing/overview.md`

## Multi-Repo

| Repo | Content |
|------|---------|
| **nixfleet** (this repo) | Framework, Rust crates, tests, docs |
| your fleet repo | Your org's fleet configuration consuming nixfleet |

## Architecture

See `ARCHITECTURE.md` for system overview, module graph, and design decisions. See `TECHNICAL.md` for Rust workspace, lifecycle states, and Nix gotchas.

Key principles:
- **mkHost** is the single API — closure over framework inputs, returns `nixosSystem`/`darwinSystem`
- **Scopes** self-activate via `lib.mkIf` on hostSpec flags — `_`-prefixed for import-tree exclusion
- **Service modules** (agent, CP, cache-server, cache, microvm-host) auto-included by mkHost, disabled by default
- **specialArgs** passes framework inputs to all modules; fleet repos extend via hostSpec and plain NixOS modules

## Critical Rules

- **Framework vs fleet:** Opinionated modules (graphical, dev, theming, dotfiles) belong in fleet repos. The framework provides lib + core + base/impermanence/agent/CP/cache/microvm.
- **Plain modules:** Scopes are plain NixOS/HM modules. They self-activate with `lib.mkIf hS.<flag>`.
- **Scope-aware impermanence:** Persist paths live alongside their program definitions, not centralized.
- **hostSpec extension:** Fleet repos extend `hostSpec` with their own flags via plain NixOS modules.
