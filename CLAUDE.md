# NixFleet

Declarative NixOS fleet management framework. Nix modules + Rust agent/control-plane/CLI.

## Structure

```
modules/
├── _shared/lib/       # Framework API: mkHost, mkVmApps
├── _shared/           # hostSpec options, disk templates
├── core/              # Core NixOS/Darwin modules (_nixos.nix, _darwin.nix)
├── scopes/            # Scope modules (_base, _impermanence, _firewall, _secrets, _backup, _monitoring, nixfleet/_agent, nixfleet/_control-plane)
├── tests/             # Eval tests, VM tests, integration tests
├── apps.nix           # Flake apps (validate, build-vm, start-vm, stop-vm, clean-vm, test-vm, provision)
├── fleet.nix          # Framework test fleet (5 hosts)
└── flake-module.nix   # Framework exports (lib.mkHost, nixosModules, diskoTemplates)
agent/                 # Rust: nixfleet-agent (state machine daemon)
control-plane/         # Rust: nixfleet-control-plane (Axum HTTP server)
cli/                   # Rust: nixfleet CLI (deploy, status, rollback)
shared/                # Rust: nixfleet-types (shared data types)
examples/
├── client-fleet/      # Example: fleet consuming mkHost via flake-parts
├── standalone-host/   # Example: single machine in its own repo
└── batch-hosts/       # Example: 50 edge devices from a template
docs/
├── adr/               # Architecture Decision Records (6 ADRs)
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
nix run .#start-vm -- -h web-02    # start VM as headless daemon
nix run .#start-vm -- --all        # start all installed VMs
nix run .#stop-vm -- -h web-02     # stop VM daemon
nix run .#clean-vm -- -h web-02    # delete VM disk + state
nix run .#test-vm -- -h web-02     # end-to-end VM test cycle
nix run .#provision -- -h web-02 --target root@192.168.1.10  # real hardware
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
| `firewall` | `!isMinimal` | SSH rate limiting, nftables, drop logging |
| `secrets` | `nixfleet.secrets.enable = true` | Identity paths, persist, boot ordering, key validation |
| `backup` | `nixfleet.backup.enable = true` | Systemd timer, hooks, health ping, status reporting |
| `monitoring` | `nixfleet.monitoring.nodeExporter.enable = true` | Node exporter with fleet-tuned collector defaults |
| `nixfleet-agent` | `services.nixfleet-agent.enable = true` | Fleet agent systemd service; key options: `metricsPort` (Prometheus listener), `metricsOpenFirewall`, `allowInsecure` |
| `nixfleet-control-plane` | `services.nixfleet-control-plane.enable = true` | Control plane HTTP server; `GET /metrics` always available on listen address; routes split: agent-facing (mTLS, no API key) vs admin (API key required); when `tls.clientCa` is set, all connections require a client certificate (defense-in-depth) |

Fleet repos add opinionated scopes (dev tools, desktop environments, theming, etc.) as plain NixOS/HM modules.

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

- **mkHost** is a closure over framework inputs (nixpkgs, home-manager, disko, impermanence)
- It calls `nixosSystem`/`darwinSystem` directly, injecting core modules and scopes
- **Scopes** are plain NixOS/HM modules (`_`-prefixed for import-tree exclusion) that self-activate via hostSpec flags
- **Service modules** (agent, CP) are auto-included by mkHost, disabled by default
- **hostSpec** provides base options; fleet repos extend with their own flags via plain NixOS modules
- **Framework inputs via specialArgs:** mkHost passes framework inputs (nixpkgs, home-manager, disko, etc.) to modules via `specialArgs = { inherit inputs; }`. Fleet repos access these as `inputs` in their modules. Fleet-specific customization uses hostSpec extensions and plain NixOS modules, not a separate input namespace.

## Critical Rules

- **Framework vs fleet:** Opinionated modules (graphical, dev, theming, dotfiles) belong in fleet repos. The framework provides lib + core + base/impermanence/agent/CP.
- **Plain modules:** Scopes are plain NixOS/HM modules. They self-activate with `lib.mkIf hS.<flag>`.
- **Scope-aware impermanence:** Persist paths live alongside their program definitions, not centralized.
- **hostSpec extension:** Fleet repos extend `hostSpec` with their own flags via plain NixOS modules.
