# NixFleet

[![CI](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml/badge.svg)](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml)
[![License: MIT/AGPL](https://img.shields.io/badge/license-MIT%2FAGPL-blue)](LICENSE-MIT)
[![Latest tag](https://img.shields.io/github/v/tag/arcanesys/nixfleet?label=version&sort=semver)](https://github.com/arcanesys/nixfleet/releases)

Declarative NixOS fleet management where **truth lives in git and signing keys, and the control plane is a caching router for already-signed intent**. Compromise of the control plane is an outage, not a breach.

## Design principle

> The control plane holds no secrets, forges no trust, and can be rebuilt from empty state without data loss.

Every component below serves that inversion. In v0.1, the control plane was the source of truth; compromise it and the fleet followed wherever it pointed. In v0.2, the truth is signed by CI from a git commit, the control plane only routes verified artifacts, and agents independently verify everything they're told to run.

## What this gets you

- **Drift impossible by construction.** A host's state is a pure function of its declaration. `nix build` is the gate; if it builds, it's already correct.
- **Sovereign by default.** No US cloud platforms in the trust path. Self-hosted Forgejo + cache + control plane. If NixFleet disappears, hosts keep running with stock NixOS tools.
- **Compliance as a release gate, not a scanner.** Static controls fail the build before a non-compliant closure can ship; runtime probes block wave promotion and can trigger rollback. (See [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance).)
- **Magic rollback.** Activate → confirm window → auto-revert on silence. Unattended canaries are safe by deadline, not by hope.
- **Reproducible supply chain.** `flake.lock` pins every input. Every closure is content-addressed and cache-signed. SBOM provenance is a property of the build, not a separate tool.
- **Atomic, instant rollback** via NixOS generation switching.
- **Darwin participation** for macOS hosts via the nix-darwin agent.

## Architecture

The runtime is a Rust stack. The **agent** runs on each managed host, polls the control plane for desired state, fetches the target NixOS closure, applies it as a new generation, and reports health back. The **control plane** is an Axum HTTP server with mTLS authentication, SQLite storage, and role-based access control. Agent identity is bound to the TLS client certificate. **Operator binaries** mint bootstrap tokens and derive trust-root pubkeys; there is no long-lived operator daemon - fleet changes are git pushes, the control plane picks them up via signed-artifact poll.

```
Operator             Forgejo (fleet repo)         Control Plane              Hosts
  |  git push           |                              |                       |
  |-------------------->|--- HTTPS poll (signed) ----->|                       |
  |                     |                              |<-- poll (mTLS) -------|
  |                     |                              |--- target closure -->|
  |                     |                              |<-- health + evidence-|
```

No imperative deploy/apply endpoints exist on the control plane. The only verb available to operators is "commit and push."

## Status - v0.2 spine

Tracked in [#10](https://github.com/abstracts33d/nixfleet/issues/10):

| Pillar | Status |
|--------|--------|
| Declarative fleet topology (`mkFleet`) | shipped |
| GitOps reconciler (commit = intent, no deploy commands) | shipped |
| Signed artifacts (CI release key, attic, host probes) | shipped |
| Freshness window (agents refuse stale targets) | shipped |
| Magic rollback (deadline-based auto-revert) | shipped |
| Compliance as rollout gate (static + runtime) | static shipped; runtime gate enforcing; CLI surfacing in flight |

## Quick start

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
        nixfleet.scopes.persistence.impermanence
        nixfleet.scopes.secrets

        ./hardware-configuration.nix
        ({ config, ... }: {
          hostSpec.userName = "deploy";
          users.users.deploy = {
            isNormalUser = true;
            extraGroups = [ "wheel" ];
            openssh.authorizedKeys.keys = [ "ssh-ed25519 AAAA..." ];
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

### Declaring a fleet

`mkFleet` takes a typed declaration of hosts, channels, edges, disruption budgets, compliance modes, and revocations. CI evaluates it, signs it, and writes `fleet.resolved.json`. The control plane polls that artifact, verifies the signature, and reconciles toward it. See [`docs/rfcs/0001-fleet-nix.md`](docs/rfcs/0001-fleet-nix.md) for the full schema.

### Deployment

Standard NixOS tooling works out of the box:

```sh
nixos-anywhere --flake .#my-server root@192.168.1.50   # Fresh install
sudo nixos-rebuild switch --flake .#my-server           # Local rebuild
darwin-rebuild switch --flake .#my-mac                  # macOS
```

Fleet rollouts are git-driven: commit → CI builds and signs → CP polls `fleet.resolved.json` → agents pull their per-host target on next checkin. There is no operator CLI verb between commit and host activation.

### Enrolling a new host

Two bootstrap subcommands of `nixfleet` (in `packages.nixfleet-cli`):

```sh
# Derive the org-root pubkey for trust.json (run once at fleet init).
nix shell nixfleet#nixfleet-cli -c \
  nixfleet derive-pubkey /path/to/org-root.ed25519.key

# Mint a one-shot bootstrap token (run once per new host).
nix shell nixfleet#nixfleet-cli -c \
  nixfleet mint-token \
    --hostname my-server \
    --csr-pubkey-fingerprint <sha256-base64-of-CSR-spki> \
    --org-root-key /path/to/org-root.ed25519.key \
    --validity-hours 24 \
  > bootstrap-token-my-server.json
```

The token is committed to the fleet repo (encrypted via your secrets backend) and consumed by the agent's first-boot `/v1/enroll` call.

### VM lifecycle (consumer fleets)

Fleets that opt into VM testing wire `nixfleet.lib.mkVmApps` into their flake's `apps`:

```nix
apps = nixfleet.lib.mkVmApps { inherit pkgs; };
```

This exposes `build-vm`, `start-vm`, `stop-vm`, `clean-vm`, `test-vm` as `nix run .#<name>` in the consumer fleet.

### Test runner

```sh
nix run .#validate              # Fast: format + flake check + eval + host builds
nix run .#validate -- --rust    # + cargo nextest + clippy + nix-sandbox builds
nix run .#validate -- --vm      # + every fleet-harness-* scenario
nix run .#validate -- --all     # Everything
```

## Operator CLI

Build the CLI:

```bash
cargo build --release -p nixfleet-cli
install -m 0755 target/release/nixfleet ~/.local/bin/
```

Initialise operator config:

```bash
nixfleet config init \
  --cp-url https://cp.example.com:8080 \
  --ca-cert /etc/nixfleet/ca.pem \
  --client-cert ~/.config/nixfleet/operator.pem \
  --client-key  ~/.config/nixfleet/operator.key
```

This writes `~/.config/nixfleet/config.toml` (mode 0600).

Inspect fleet state:

```bash
# rendered table
nixfleet status

# raw HostsResponse JSON
nixfleet status --json

# wave-by-wave history
nixfleet rollout trace <rollout-id>
```

Override config-file values per-invocation with `--cp-url`, `--ca-cert`, `--client-cert`, `--client-key`, or the matching `NIXFLEET_*` env vars.

## Ecosystem

| Repository | What it provides | License |
|------------|-----------------|---------|
| **nixfleet** (this repo) | Framework: `mkHost` / `mkFleet`, contract impls (`flake.scopes.*`), agent, control plane, operator helpers | MIT / AGPL |
| [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) | Typed compliance controls (NIS2, DORA, ISO 27001, ANSSI), signed evidence, the rollout-gate moat | MIT |
| [nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) | Reference 6-host QEMU fleet with pre-baked credentials | MIT |

The framework ships kernel + contract impls. Service wraps, hardware bundles, role taxonomies, and other deployment opinions live in the consuming fleet repo - not in nixfleet - so the framework stays generic and the consumer keeps full ownership of its shape.

> **Try it now:** [nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) ships a complete 6-host QEMU fleet. Clone, build VMs, deploy.

## Non-goals

- **Not a general-purpose imperative runner.** No "run this script on all hosts" - the only vocabulary is "target closure hash."
- **Not a multi-tenant SaaS.** Single administrative domain.
- **Not a replacement for NixOS tooling.** `nixos-rebuild`, `nix flake`, `nix-store --verify` remain ground truth.
- **Not a cloud provisioning tool.** Fleet membership is declared; hosts aren't auto-created from templates.
- **Not agentless.** Pull-based means an agent runs on every managed host. Acceptable cost for the sovereignty property.

## Documentation

- Full docs: [arcanesys.github.io/nixfleet](https://arcanesys.github.io/nixfleet)
- Architecture: [`docs/design/architecture.md`](docs/design/architecture.md)
- RFCs: [`docs/rfcs/`](docs/rfcs/) - fleet.nix schema, reconciler, wire protocol, attestation, trust lifecycle

## Development

```sh
nix develop                        # Dev shell (cargo, clippy, rustfmt, rust-analyzer, tokei, cloc)
nix fmt                            # Format (alejandra + rustfmt + shfmt)
nix run .#validate -- --all        # Full test suite
tools/loc.sh                       # Rust LOC report (prod / inline-tests / integ-tests)
tools/loc.sh --update              # Refresh tools/loc-baseline.txt after intentional changes
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for contributor guidelines.

## License

Framework, agent, and CLI: [MIT](LICENSE-MIT). Control plane: [AGPL-3.0](LICENSE-AGPL).

Your fleet configurations, custom modules, and agent deployments remain fully private - the AGPL applies only to modifications of the control plane itself.
