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
├── fleet.nix          # Framework test fleet (8 hosts)
└── flake-module.nix   # Framework exports (lib.mkHost, nixosModules, diskoTemplates)
agent/                 # Rust: nixfleet-agent (sequential deploy cycle daemon)
control-plane/         # Rust: nixfleet-control-plane (Axum HTTP server)
cli/                   # Rust: nixfleet CLI (deploy, status, rollback)
shared/                # Rust: nixfleet-types (shared data types)
examples/
├── client-fleet/      # Example: fleet consuming mkHost via flake-parts
├── standalone-host/   # Example: single machine in its own repo
└── batch-hosts/       # Example: 50 edge devices from a template
docs/
├── adr/               # Architecture Decision Records (8 ADRs)
└── mdbook/            # Technical reference + user guide (mdbook)
```

## Commands

```sh
# Nix
nix develop                        # dev shell
nix fmt                            # format (alejandra + shfmt)
nix flake check --no-build         # eval tests (instant)
nix run .#validate                 # full validation (eval + host builds)
nix run .#validate -- --vm         # include VM tests (slow)
nix build .#checks.x86_64-linux.vm-fleet --no-link  # 4-node fleet test (CP + 3 agents, TLS/mTLS, rollouts)
nix run .#build-vm -- -h web-02    # install VM (ISO + nixos-anywhere)
nix run .#build-vm -- --all        # install all hosts
nix run .#build-vm -- --all --vlan 1234  # install all with inter-VM VLAN
nix run .#start-vm -- -h web-02    # start VM as headless daemon
nix run .#start-vm -- --all        # start all installed VMs
nix run .#start-vm -- --all --vlan 1234  # start all with inter-VM VLAN
nix run .#stop-vm -- -h web-02     # stop VM daemon
nix run .#clean-vm -- -h web-02    # delete VM disk + state
nix run .#test-vm -- -h web-02     # end-to-end VM test cycle
nix build .#iso                    # custom installer ISO

# Deployment (standard NixOS tooling — no custom scripts)
nixos-anywhere --flake .#hostname root@ip       # fresh install
sudo nixos-rebuild switch --flake .#hostname    # local rebuild
nixos-rebuild switch --flake .#hostname --target-host root@ip  # remote rebuild
darwin-rebuild switch --flake .#hostname        # macOS rebuild

# Rust
cargo test --workspace             # all Rust tests
cargo build --workspace            # build all crates
cargo clippy --workspace           # lint

# Release management
nixfleet release create --push-to s3://my-fleet-cache  # build all hosts, push to cache, register release
nixfleet release create --copy                         # build all hosts, copy via SSH, register release
nixfleet release create --dry-run                 # build and show manifest only
nixfleet release list                             # list recent releases
nixfleet release show <RELEASE_ID>                # show release details and per-host entries
nixfleet release diff <ID_A> <ID_B>               # diff two releases
```

## Framework API

| Function | Purpose |
|----------|---------|
| `nixfleet.lib.mkHost` | Single host definition -> returns `nixosSystem` or `darwinSystem` |
| `nixfleet.lib.mkVmApps` | VM helper apps generator for fleet repos |

### mkHost Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `hostName` | string | yes | Machine hostname |
| `platform` | string | yes | `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin` |
| `stateVersion` | string | no | NixOS/Darwin state version (default: "24.11") |
| `hostSpec` | attrset | no | Host configuration flags (extensible by fleet modules) |
| `modules` | list | no | Additional NixOS/Darwin modules |
| `isVm` | bool | no | Inject QEMU VM hardware (default: false) |

### Exports

```nix
nixfleet.lib.mkHost                              # the API
nixfleet.lib.mkVmApps                            # VM helper generator
nixfleet.nixosModules.nixfleet-core              # raw NixOS core module
nixfleet.diskoTemplates.btrfs                    # standard btrfs disk template
nixfleet.diskoTemplates.btrfs-impermanence       # btrfs with impermanence layout
nixfleet.packages.${system}.iso                  # custom installer ISO
nixfleet.packages.${system}.nixfleet-agent       # Rust agent binary
nixfleet.packages.${system}.nixfleet-control-plane # Rust control-plane binary
nixfleet.packages.${system}.nixfleet-cli         # Rust CLI binary
```

## Framework Scopes

Scopes are plain NixOS/HM modules auto-included by mkHost. They self-activate via `lib.mkIf` on hostSpec flags.

| Scope | Flag / Enable condition | What it provides |
|-------|------------------------|-----------------|
| `base` | `!isMinimal` | Universal CLI packages (NixOS + Darwin + HM) |
| `impermanence` | `isImpermanent` | Btrfs root wipe + system/user persistence paths |
| `firewall` | `!isMinimal` | SSH rate limiting, nftables, drop logging; bridge forwarding rules when microVM host is enabled |
| `secrets` | `nixfleet.secrets.enable = true` | Identity paths, persist, boot ordering, key validation |
| `backup` | `nixfleet.backup.enable = true` | Systemd timer, hooks, health ping, status reporting; optional `backend` (`"restic"`, `"borgbackup"`) for concrete backup commands |
| `monitoring` | `nixfleet.monitoring.nodeExporter.enable = true` | Node exporter with fleet-tuned collector defaults |
| `nixfleet-agent` | `services.nixfleet-agent.enable = true` | Fleet agent systemd service; key options: `metricsPort` (Prometheus listener), `metricsOpenFirewall`, `allowInsecure`. Tags auto-synced to CP via health reports. Verifies store path exists locally via `nix path-info` when no cache URL is configured. Poll interval adapts via `poll_hint` from CP (5s during active rollouts). |
| `nixfleet-control-plane` | `services.nixfleet-control-plane.enable = true` | Control plane HTTP server; `GET /metrics` always available on listen address; routes split: agent-facing (mTLS, no API key) vs admin (API key required); when `tls.clientCa` is set, all connections require a client certificate (defense-in-depth) |
| `nixfleet-cache-server` | `services.nixfleet-cache-server.enable = true` | Harmonia binary cache server; serves from local Nix store; key options: `port`, `signingKeyFile`, `openFirewall` |
| `nixfleet-cache` | `services.nixfleet-cache.enable = true` | Binary cache client; configures `nix.settings.substituters` + `trusted-public-keys` |
| `nixfleet-microvm-host` | `services.nixfleet-microvm-host.enable = true` | MicroVM host with TAP + bridge networking, DHCP (dnsmasq), NAT; microVMs participate in fleet as first-class members |

Fleet repos add opinionated scopes (dev tools, desktop environments, theming, etc.) as plain NixOS/HM modules.

## CLI

```bash
# Initialize CLI config
nixfleet init --control-plane-url https://lab:8080 --ca-cert modules/_config/fleet-ca.pem

# Bootstrap first API key
API_KEY=$(nixfleet bootstrap \
  --control-plane-url https://cp:8080 \
  --client-cert cp-cert --client-key cp-key --ca-cert fleet-ca.pem)

# Fleet status
nixfleet status
nixfleet machines list --tag web

# Deploy via control plane (requires a release)
nixfleet deploy --release rel-abc123 --tag web --strategy canary --wait
nixfleet deploy --push-to s3://my-fleet-cache --tag web --strategy canary --wait  # implicit release creation
nixfleet deploy --copy --tag web --strategy staged --wait              # implicit release, SSH copy

# Direct SSH deploy (no control plane needed)
nixfleet deploy --hosts web-02 --ssh                                          # deploy via SSH (resolves hostname)
nixfleet deploy --hosts web-02 --ssh --target root@192.168.1.10               # deploy via SSH to specific IP

# Rollout management
nixfleet rollout list
nixfleet rollout status <ID>       # includes events timeline
nixfleet rollout resume <ID>
nixfleet rollout cancel <ID>
```

mTLS flags (`--client-cert`, `--client-key`, `--ca-cert`) and `--api-key` can be set via env vars: `NIXFLEET_CLIENT_CERT`, `NIXFLEET_CLIENT_KEY`, `NIXFLEET_CA_CERT`, `NIXFLEET_API_KEY`.

### Rollout Events

Every rollout state transition (created → running → paused → completed, batch started/completed/failed) is recorded as an event in the `rollout_events` table. Events include timestamp, type, detail JSON, and actor. The `rollout status` CLI command shows these as a timeline.

### Rollout Executor & Generation Gating

The rollout executor verifies that each agent's `current_generation` matches the release entry before accepting health reports during batch evaluation. This ensures agents deployed from the same release can be evaluated together for health status. Mismatched generations are flagged as out-of-sync and paused until manually resumed or the rollout is cancelled.

### Configuration

The CLI reads settings from three sources (highest priority wins):

1. CLI flags (`--control-plane-url`, `--api-key`, etc.)
2. Environment variables (`NIXFLEET_API_KEY`, `NIXFLEET_CA_CERT`, etc.)
3. `~/.config/nixfleet/credentials.toml` — API keys (auto-saved by `nixfleet bootstrap`)
4. `.nixfleet.toml` — connection settings (created by `nixfleet init`, committed to fleet repo)

#### `.nixfleet.toml` example

```toml
[control-plane]
url = "https://lab:8080"
ca-cert = "modules/_config/fleet-ca.pem"

[tls]
client-cert = "/run/agenix/agent-${HOSTNAME}-cert"
client-key = "/run/agenix/agent-${HOSTNAME}-key"

[cache]
url = "http://lab:5000"
push-to = "ssh://root@lab"

[deploy]
strategy = "staged"
health-timeout = 300
```

Setup:

```sh
nixfleet init --control-plane-url https://lab:8080 --ca-cert modules/_config/fleet-ca.pem
nixfleet bootstrap    # auto-saves API key to ~/.config/nixfleet/credentials.toml
```

## Control Plane API

### Bootstrap

```bash
# Via CLI (recommended)
API_KEY=$(nixfleet bootstrap --client-cert cp-cert --client-key cp-key --ca-cert fleet-ca.pem)

# Via curl
curl -X POST https://cp:8080/api/v1/keys/bootstrap \
  --cacert fleet-ca.pem --cert cp-cert --key cp-key \
  -H 'Content-Type: application/json' -d '{"name":"admin"}'
# Returns: {"key":"nfk-...","name":"admin","role":"admin"}
# Returns 409 if keys already exist
```

### API Endpoints (new)

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/api/v1/releases` | deploy | Create a release from a manifest |
| GET | `/api/v1/releases` | readonly | List releases (paginated, newest first) |
| GET | `/api/v1/releases/{id}` | readonly | Get release with entries |
| GET | `/api/v1/releases/{id}/diff/{other_id}` | readonly | Diff two releases |
| DELETE | `/api/v1/releases/{id}` | admin | Delete a release (only if no rollout references it) |

The `POST /api/v1/rollouts` endpoint requires a `release_id` field. The `GET /api/v1/rollouts/{id}` response includes `events` (timeline).

### Agent tag sync

Agent tags (from `services.nixfleet-agent.tags`) are automatically synced to the CP on every health report. No manual `POST /tags` needed — tags are self-managing from NixOS config.

## Consuming the Framework

```nix
# Minimal fleet repo — flake.nix (no flake-parts needed)
{
  inputs.nixfleet.url = "github:your-org/nixfleet";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";

  outputs = {nixfleet, ...}: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = { userName = "alice"; timeZone = "US/Eastern"; };
      modules = [ ./hardware-configuration.nix ./disk-config.nix ];
    };
  };
}
```

See `examples/` for standalone-host, batch-hosts, and client-fleet patterns.

## Testing

3-tier pyramid:
- **Eval** (`modules/tests/eval.nix`) — config correctness, instant. `nix flake check --no-build`
- **VM** (`modules/tests/vm.nix`, `vm-nixfleet.nix`) — runtime assertions. `nix run .#validate -- --vm`
- **VM Infrastructure** (`modules/tests/vm-infra.nix`) — firewall, node exporter, backup timer, secrets key generation. `nix run .#validate -- --vm`
- **VM Fleet** (`modules/tests/vm-fleet.nix`) — 4-node fleet test (CP + 3 agents) with required mTLS, canary rollout on web tag (passes), all-at-once on db tag (pauses on health gate failure), pause/resume. `nix build .#checks.x86_64-linux.vm-fleet --no-link`
- **Integration** (`modules/tests/integration/`) — mock client consumption pattern

## Multi-Repo

| Repo | Content |
|------|---------|
| **nixfleet** (this repo) | Framework, Rust crates, tests, docs |
| your fleet repo | Your org's fleet configuration consuming nixfleet |

## Architecture

- **mkHost** is a closure over framework inputs (nixpkgs, home-manager, disko, impermanence, microvm)
- It calls `nixosSystem`/`darwinSystem` directly, injecting core modules and scopes
- **Scopes** are plain NixOS/HM modules (`_`-prefixed for import-tree exclusion) that self-activate via hostSpec flags
- **Service modules** (agent, CP, cache-server, cache, microvm-host) are auto-included by mkHost, disabled by default
- **hostSpec** provides base options; fleet repos extend with their own flags via plain NixOS modules
- **Framework inputs via specialArgs:** mkHost passes framework inputs (nixpkgs, home-manager, disko, etc.) to modules via `specialArgs = { inherit inputs; }`. Fleet repos access these as `inputs` in their modules. Fleet-specific customization uses hostSpec extensions and plain NixOS modules, not a separate input namespace.

## Critical Rules

- **Framework vs fleet:** Opinionated modules (graphical, dev, theming, dotfiles) belong in fleet repos. The framework provides lib + core + base/impermanence/agent/CP/cache/microvm.
- **Plain modules:** Scopes are plain NixOS/HM modules. They self-activate with `lib.mkIf hS.<flag>`.
- **Scope-aware impermanence:** Persist paths live alongside their program definitions, not centralized.
- **hostSpec extension:** Fleet repos extend `hostSpec` with their own flags via plain NixOS modules.
