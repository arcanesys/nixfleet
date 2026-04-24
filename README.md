# NixFleet

[![CI](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml/badge.svg)](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml)
[![License: MIT/AGPL](https://img.shields.io/badge/license-MIT%2FAGPL-blue)](LICENSE-MIT)
[![v0.1.0](https://img.shields.io/github/v/tag/arcanesys/nixfleet?label=version)](https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0)

Declarative NixOS fleet management with reproducible deployments, cryptographic security, and compliance automation.

![NixFleet Demo](docs/assets/nixfleet-demo.gif)

## Why NixFleet

Infrastructure teams face four converging crises:

- **Configuration drift** - Imperative tools (Ansible, Puppet, Chef) depend on existing system state. Every command may produce a different result depending on what ran before. State diverges silently over time.
- **Sovereignty** - Fleet management depends on US cloud platforms (Jamf, Intune, AWS SSM), creating legal exposure under GDPR, the Cloud Act, and European digital sovereignty doctrine.
- **Bolted-on security** - Security is layered after the fact (EDR agents, SIEM collectors, SBOM scanners) rather than built into the system model. No tool can prove the running system matches its declared state.
- **Compliance** - Frameworks like NIS2, DORA, ISO 27001, and ANSSI require traceability, rapid incident recovery, and supply chain security that traditional stacks cannot prove.

NixFleet resolves all four by building on NixOS's declarative model. Infrastructure is a pure function of its declaration, so drift is impossible by construction. The hash-addressed Nix store makes every binary immutable and verifiable. Impermanence erases non-persistent state at reboot. `flake.lock` pins every dependency with cryptographic hashes, providing automatic SBOM and supply chain provenance. Every deployment is a Git commit. Rollback is atomic and instant. The entire stack is self-hosted and open source - if NixFleet disappears, your machines keep running with standard NixOS tools.

## Architecture

NixFleet's runtime is a Rust stack. The **agent** runs on each managed host - it polls the control plane for desired state, fetches the target NixOS closure, applies it as a new generation, and reports health back. The **control plane** is an Axum HTTP server with mTLS authentication, SQLite storage, and role-based access control. Agent identity is derived from the TLS client certificate CN. The **CLI** is the operator interface for deployments, rollouts, status checks, and rollback.

```
Operator                Control Plane              Hosts
  |                         |                        |
  |-- deploy/rollout ------>|                        |
  |                         |<-- poll (mTLS) --------|
  |                         |--- desired state ----->|
  |                         |<-- health report ------|
  |<-- status --------------|                        |
```

## Ecosystem

| Repository | What it provides | License |
|------------|-----------------|---------|
| **nixfleet** (this repo) | Framework: `mkHost` API, agent, control plane, CLI | MIT / AGPL |
| [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes) | 17 infrastructure scopes, 4 roles, 6 disk templates | MIT |
| [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) | 16 compliance controls, 4 regulatory frameworks, evidence probes | MIT |

nixfleet provides mechanism, nixfleet-scopes provides generic infrastructure opinions, nixfleet-compliance adds regulatory controls. Each works standalone.

> **Try it now:** [nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) ships a complete 6-host QEMU fleet with pre-baked credentials. Clone, build VMs, deploy - no setup required.

## Quick Start

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixfleet.url = "github:arcanesys/nixfleet";
  };

  outputs = { nixpkgs, nixfleet, ... }: {
    nixosConfigurations.my-server = nixfleet.lib.mkHost {
      hostName = "my-server";
      platform = "x86_64-linux";
      modules = [
        nixfleet.scopes.roles.server
        ./hardware-configuration.nix
        ({ ... }: {
          nixfleet.operators = {
            primaryUser = "deploy";
            users.deploy = {
              isAdmin = true;
              sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
            };
          };
          services.nixfleet-agent = {
            enable = true;
            controlPlane.url = "https://cp.example.com:8080";
          };
        })
      ];
    };
  };
}
```

For starter templates, run `nix flake init -t github:arcanesys/nixfleet` (standalone), `#batch` (identical machines), or `#fleet` (multi-host).

### Deployment

Standard NixOS tooling works out of the box:

```sh
nixos-anywhere --flake .#my-server root@192.168.1.50   # Fresh install (formats disks)
sudo nixos-rebuild switch --flake .#my-server           # Local rebuild
darwin-rebuild switch --flake .#my-mac                  # macOS
```

With the control plane, use the CLI for fleet operations:

```sh
nixfleet deploy --tags production --push-to ssh://root@cache   # Build, push, register release
nixfleet rollout start --release latest --strategy canary       # Staged rollout with health gates
nixfleet status                                                 # Fleet-wide health overview
```

### Shell completions

```sh
eval "$(nixfleet completions zsh)"   # Zsh
eval "$(COMPLETE=bash nixfleet)"     # Bash
```

Dynamic tab completion for rollout/release/machine IDs, queried live from the control plane.

## Features

- **Fleet orchestration** - Agent polls control plane for desired state, applies NixOS generations, reports health
- **Deployment strategies** - Canary, staged, and all-at-once rollouts with health gates and automatic rollback
- **Operators** - Declarative multi-user management with SSH keys, sudo access, Home Manager routing
- **Compliance as code** - NIS2, DORA, ISO 27001, ANSSI controls with evidence probes and governance engine
- **Securix compatibility** - Integrates with [Securix](https://github.com/arcanesys/securix), the DINUM-aligned secure NixOS distribution for French and European government environments. See `examples/securix-endpoint/` for an ANSSI BP-028 hardened endpoint.
- **Instant rollback** - Atomic NixOS generation switching
- **Darwin support** - macOS fleet participation via nix-darwin agent

## Examples

| Example | When to use it |
|---------|---------------|
| `examples/standalone-host/` | Single machine in its own repo |
| `examples/batch-hosts/` | 50+ identical machines from a template |
| `examples/client-fleet/` | Multi-host fleet with flake-parts |
| `examples/fleet-homelab/` | Declarative fleet via `lib.mkFleet` — run `nix eval .#fleet.resolved --json` to inspect the resolved artifact |
| `examples/securix-endpoint/` | Hardened ANSSI BP-028 distro composition with Securix |

## Documentation

Full documentation: [arcanesys.github.io/nixfleet](https://arcanesys.github.io/nixfleet)

## Development

```sh
nix develop                        # Dev shell (cargo, clippy, rustfmt, rust-analyzer)
nix fmt                            # Format (alejandra + rustfmt + shfmt)
nix run .#validate -- --all        # Full test suite (format, eval, hosts, VM, Rust, clippy)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed contributor guidelines.

## License

Framework, agent, and CLI: [MIT](LICENSE-MIT). Control plane: [AGPL-3.0](LICENSE-AGPL).

Your fleet configurations, custom modules, and agent deployments remain fully private - the AGPL applies only to modifications of the control plane itself.
