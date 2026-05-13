# NixFleet

[![CI](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml/badge.svg)](https://github.com/arcanesys/nixfleet/actions/workflows/ci.yml)
[![License: MIT/AGPL](https://img.shields.io/badge/license-MIT%2FAGPL-blue)](LICENSE-MIT)
[![Latest tag](https://img.shields.io/github/v/tag/arcanesys/nixfleet?label=version&sort=semver)](https://github.com/arcanesys/nixfleet/releases)

Declarative NixOS fleet management with signed GitOps. Truth lives in git and signing keys; the control plane is a caching router for already-signed intent. **Compromise of the control plane is an outage, not a breach.**

> The control plane holds no secrets, forges no trust, and can be rebuilt from empty state without data loss.

## Who this is for

You operate 10-200 servers under NIS2 / DORA / ISO 27001 / ANSSI BP-028, or you're planning the regulated zone of a fleet you intend to bring under those frameworks. You don't have to be on NixOS yet - pilot scope can include the NixOS layer.

You need:

- An auditor-grade evidence chain you can produce on demand, without trusting your scanner vendor
- A deploy path that **refuses** non-compliant closures before they ship
- Atomic rollback when activation fails - not a postmortem after pages
- No US cloud platform in the trust path
- One operator, not five tools

## What changes

- **Drift is impossible by construction.** A host's state is a pure function of its declaration. `nix build` is the gate; if it builds, it's correct.
- **The control plane holds no signing keys.** Compromising it grants an attacker zero deploy authority. Agents reject anything not signed by the CI release key.
- **Compliance is a release gate, not a scanner.** Static predicates fail the build; runtime probes block wave promotion and trigger rollback. (See [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance).)
- **Magic rollback.** Activate -> confirm window -> auto-revert on silence. Unattended canaries are safe by deadline, not by hope.
- **Sovereign by default.** Self-hosted Git forge, Nix binary cache, control plane. If NixFleet disappears, hosts keep running with stock NixOS tools.
- **Reproducible supply chain.** `flake.lock` pins every input; every closure is content-addressed and cache-signed. SBOM provenance is a property of the build.
- **Darwin participation** for macOS hosts via the nix-darwin agent.

## See it work

[nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) boots a 4-VM reference fleet on your laptop in ~10 minutes, exercises the canonical GitOps loop end-to-end, and lets you trigger a signed wave promotion and a magic rollback by editing one config.

## Pilot

We run free 12-week pilots for regulated operators (typical fleet 10-200 hosts). Pilot scope covers your **regulated zone** - 5 to 15 hosts, existing NixOS or migrated from Ansible / Puppet / Chef during the 12 weeks. The deliverable is a working signed-GitOps fleet on that zone plus an auditor-ready evidence packet at month 3. The rest of your infrastructure stays where it is.

Details, scope, and what we ask for in return: <https://arcanesys.fr/en/pilot>.

Contact: <contact@arcanesys.fr>

## Ecosystem

| Repository | Purpose | License |
|------------|---------|---------|
| **nixfleet** (this repo) | Framework: `mkHost` / `mkFleet`, contract impls (`flake.scopes.*`), agent, control plane, operator CLI | MIT / AGPL |
| [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) | Typed compliance controls (NIS2, DORA, ISO 27001, ANSSI BP-028), signed evidence, the rollout-gate moat | MIT |
| [nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) | 4-VM reference fleet - clone, build, deploy | MIT |

The framework ships kernel + contract impls. Service wraps, hardware bundles, role taxonomies, and other deployment opinions live in the consuming fleet repo - not in nixfleet - so the framework stays generic and the consumer keeps full ownership of its shape.

## Non-goals

- **Not a general-purpose imperative runner.** The only verb is "target closure hash."
- **Not a multi-tenant SaaS.** Single administrative domain.
- **Not a replacement for NixOS tooling.** `nixos-rebuild`, `nix flake`, `nix-store --verify` remain ground truth.
- **Not a cloud provisioning tool.** Fleet membership is declared; hosts aren't auto-created from templates.
- **Not agentless.** Pull-based means an agent runs on every managed host. Acceptable cost for the sovereignty property.

## Documentation

- Full docs: [arcanesys.github.io/nixfleet](https://arcanesys.github.io/nixfleet)
- Architecture: [`docs/design/architecture.md`](docs/design/architecture.md)
- RFCs: [`docs/rfcs/`](docs/rfcs/) - `fleet.nix` schema, reconciler, wire protocol, hardware-rooted trust, trust lifecycle, freshness policy, air-gapped operation

## Development

```sh
nix develop                        # cargo, clippy, rustfmt, rust-analyzer
nix fmt                            # alejandra + rustfmt + shfmt
nix run .#validate -- --all        # full test suite
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Framework, agent, and CLI: [MIT](LICENSE-MIT). Control plane: [AGPL-3.0](LICENSE-AGPL).

Your fleet configurations, custom modules, and agent deployments remain fully private - the AGPL applies only to modifications of the control plane itself.
