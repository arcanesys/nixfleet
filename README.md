# NixFleet

**Declarative NixOS fleet management.** Define your infrastructure as code with reproducible builds, instant rollback, and zero config drift.

## What is NixFleet?

NixFleet is a framework for managing fleets of NixOS and macOS machines. It provides:
- **`mkHost`** — single function that returns a standard `nixosSystem` or `darwinSystem`. Mechanism only — opinions come from consumer-imported modules.
- **hostSpec** — identity-only host spec (hostName, userName, home, timeZone, locale, SSH keys, password files, networking). Fleet repos extend it with their own additions.
- **Core modules** — nix settings, SSH hardening, identity pass-through (time / locale / hostName / xkb)
- **Agent + Control Plane** — Rust-based fleet orchestration with staged rollouts, health checks, and automatic rollback
- **Companion scopes** — [`arcanesys/nixfleet-scopes`](https://github.com/arcanesys/nixfleet-scopes) ships reusable `base` / `firewall` / `secrets` / `backup` / `monitoring` / `impermanence` / `home-manager` / `disko` scopes plus 4 generic roles (`server`, `workstation`, `endpoint`, `microvm-guest`). `nixos-hardware`, Sécurix-style distros, and per-fleet hardware bundles supply hardware modules.

## Quick Start

```nix
# flake.nix — single machine
{
  inputs.nixfleet.url = "github:your-org/nixfleet";
  inputs.nixfleet-scopes.url = "github:arcanesys/nixfleet-scopes";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";

  outputs = {nixfleet, nixfleet-scopes, ...}@inputs: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "alice";
        timeZone = "US/Eastern";
        sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
      };
      modules = [
        nixfleet-scopes.scopes.roles.workstation   # base + firewall + secrets + HM + backup + user
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

- **Releases** — immutable manifests mapping each host to its built store path, enabling heterogeneous fleet deployment where every machine's closure is different
- **Machine tags** — group machines for targeted operations
- **Health checks** — declarative systemd, HTTP, and command checks, plus a generation gate in the executor that prevents false-positive rollout completion from stale health reports
- **Rollout strategies** — canary, staged, all-at-once with automatic pause/revert
- **Adaptive polling** — agents react to new rollouts within seconds via `poll_hint` from the control plane
- **CLI** — `nixfleet deploy --push-to ssh://root@cache --tags production --strategy canary --wait` (builds, pushes to cache, registers a release, triggers a rollout)

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
nix run .#build-vm -- -h web-02              # install VM (ISO + nixos-anywhere)
nix run .#build-vm -- --all                  # install all hosts
nix run .#start-vm -- -h web-02              # start VM as headless daemon
nix run .#stop-vm -- -h web-02               # stop VM daemon
nix run .#clean-vm -- -h web-02              # delete VM disk + state
nix run .#test-vm -- -h web-02               # end-to-end VM test cycle
```

Fleet repos wire these via `nixfleet.lib.mkVmApps { inherit pkgs; }`.

## Development

```sh
nix develop                        # dev shell (cargo, clippy, rustfmt, rust-analyzer)
nix fmt                            # format (alejandra + rustfmt + shfmt)
nix run .#validate -- --all        # run the whole test suite (format, eval, hosts, VM, Rust, clippy, package builds)
```

`nix run .#validate -- --all` is the single entry point — prefer it over
running individual `cargo test` / `nix build .#checks` invocations. See
`docs/mdbook/testing/overview.md` for what each tier contains and how to
drill down into a specific failure.

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed contributor guidelines.

## License

nixfleet uses a dual-license model:

- **Control Plane** (`control-plane/`): [AGPL-3.0](LICENSE-AGPL) — modifications to the control plane must be shared when provided as a service
- **Everything else** (framework, agent, CLI, modules): [MIT](LICENSE-MIT) — use freely, no copyleft obligation

This means you can freely use the framework to manage your fleet without any copyleft requirement. Your fleet configurations, custom modules, and agent deployments remain fully private.
