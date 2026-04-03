# NixFleet

**Declarative NixOS fleet management.** Define your infrastructure as code with reproducible builds, instant rollback, and zero config drift.

## What is NixFleet?

NixFleet is a framework for managing fleets of NixOS and macOS machines. It provides:
- **`mkHost`** — single function that returns a standard `nixosSystem` or `darwinSystem`
- **hostSpec** — extensible host configuration flags (fleet repos add their own)
- **Core modules** — nix settings, boot, SSH hardening, networking, user management
- **Disko templates** — reusable disk layout configurations
- **Agent + Control Plane** — Rust-based fleet orchestration with staged rollouts, health checks, and automatic rollback

## Quick Start

```nix
# flake.nix — single machine, no ceremony
{
  inputs.nixfleet.url = "github:your-org/nixfleet";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";

  outputs = {nixfleet, ...}: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "alice";
        timeZone = "US/Eastern";
        sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
      };
      modules = [
        ./hardware-configuration.nix
        ./disk-config.nix
      ];
    };
  };
}
```

Deploy:
```sh
nixos-anywhere --flake .#myhost root@192.168.1.50   # fresh install
sudo nixos-rebuild switch --flake .#myhost           # rebuild
```

See `examples/` for more patterns, or use `nix flake init -t nixfleet#fleet` for a multi-host template.

## Documentation

Full documentation: [your-org.github.io/nixfleet](https://your-org.github.io/nixfleet/)

- [Quick Start](https://your-org.github.io/nixfleet/guide/getting-started/quick-start.html) — first fleet in 15 minutes
- [Deploying](https://your-org.github.io/nixfleet/guide/deploying/rollouts.html) — rollout strategies, health checks
- [ADRs](docs/adr/) — architecture decision records

## Layout

```
modules/
├── _shared/lib/       # Framework API: mkHost, mkVmApps
├── _shared/           # hostSpec options, disk templates
├── core/              # Core NixOS/Darwin modules
├── scopes/            # Scope modules (base, impermanence, agent, control-plane)
├── tests/             # Eval tests, VM tests, integration tests
├── apps.nix           # Flake apps (validate, VM helpers)
├── fleet.nix          # Test fleet for framework CI
└── flake-module.nix   # Framework exports
examples/
├── standalone-host/   # Single machine in its own repo
├── batch-hosts/       # 50 edge devices from a template
└── client-fleet/      # Fleet consuming mkHost via flake-parts
```

## Scope Pattern

mkHost auto-includes framework scopes. They self-activate based on hostSpec flags:

```nix
# isImpermanent = true -> impermanence scope activates (btrfs wipe, persistence paths)
# isMinimal = true -> base scope skips optional packages
# services.nixfleet-agent.enable = true -> agent service starts
```

The framework includes infrastructure scopes for firewall hardening (nftables, SSH rate limiting), secrets wiring (backend-agnostic identity paths), backup scaffolding (systemd timers, health pings), and monitoring (Prometheus node exporter). The agent and control plane expose Prometheus metrics endpoints for fleet-wide observability.

Fleet repos add their own scopes (dev tools, desktop environments, theming, etc.) as plain NixOS/HM modules.

## Fleet Orchestration

The agent + control plane provide fleet-wide deployment orchestration:

- **Machine tags** — group machines for targeted operations
- **Health checks** — declarative systemd, HTTP, and command checks
- **Rollout strategies** — canary, staged, all-at-once with automatic pause/revert
- **CLI** — `nixfleet deploy --tag production --strategy canary --wait`

## Deployment

Standard NixOS tooling — no custom scripts:

```sh
nixos-anywhere --flake .#hostname root@ip              # fresh install (formats disks via disko)
sudo nixos-rebuild switch --flake .#hostname            # local rebuild
nixos-rebuild switch --flake .#hostname --target-host root@ip  # remote rebuild
darwin-rebuild switch --flake .#hostname                # macOS
```

## Virtual Machines

```sh
nix run .#spawn-qemu -- --iso iso/nixos-x86_64.iso   # first boot from ISO
nix run .#spawn-qemu                                   # subsequent boots
nix run .#spawn-qemu -- --persistent -h web-02         # build + install + launch (graphical)
nix run .#test-vm -- -h web-02                         # full VM test cycle
```

Fleet repos wire these via `nixfleet.lib.mkVmApps { inherit pkgs; }`.

## Development

```sh
nix develop                        # dev shell
nix flake check --no-build         # eval tests (instant)
nix run .#validate                 # full validation (eval + host builds)
nix run .#validate -- --vm         # include VM tests
nix fmt                            # format (alejandra + shfmt)
cargo test --workspace             # Rust tests
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed contributor guidelines.

## License

nixfleet uses a dual-license model:

- **Control Plane** (`control-plane/`): [AGPL-3.0](LICENSE-AGPL) — modifications to the control plane must be shared when provided as a service
- **Everything else** (framework, agent, CLI, modules): [MIT](LICENSE-MIT) — use freely, no copyleft obligation

This means you can freely use the framework to manage your fleet without any copyleft requirement. Your fleet configurations, custom modules, and agent deployments remain fully private.
