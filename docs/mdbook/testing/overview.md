# Testing Overview

nixfleet has four test tiers that together cover configuration, Rust code,
Nix module wiring, and full multi-node runtime behaviour. This page gives you
the complete inventory and the one command that runs everything.

## Quick reference

| Command | Runs | Typical duration |
|---|---|---|
| `nix run .#validate` | Format + eval tests + all host builds | ~30 s |
| `nix run .#validate -- --vm` | ^ + every `vm-*` check (dynamically discovered) | ~20–40 min |
| `nix run .#validate -- --rust` | ^ + `cargo test --workspace` | ~3–5 min |
| `nix run .#validate -- --all` | Everything | ~25–45 min |

Individual tests:

```sh
nix flake check --no-build                                    # Tier C only
nix build .#checks.x86_64-linux.<name> --no-link              # single VM/eval check
cargo test -p nixfleet-control-plane                          # single Rust crate
cargo test -p nixfleet-control-plane --test polling_scenarios # single Rust scenario file
```

## Tier C — Eval tests (fast, ~seconds)

Pure Nix evaluations. No VMs, no Rust builds. Asserts structural properties
of `hostSpec`, scope modules, and service wiring. See
[Eval Tests](eval-tests.md) for the per-check list.

## Tier B — Integration tests (medium)

| Check | Purpose |
|---|---|
| `integration-mock-client` | Simulates a consumer flake importing `nixfleet.lib.mkHost`. Proves the public API is reachable, produces valid `nixosConfigurations`, and exposes core modules/scopes. |

## Tier A — VM tests (slow, minutes per test)

Real NixOS VMs booted under QEMU with Python test scripts driving assertions.
See [VM Tests](vm-tests.md) for the full list and per-scenario semantics,
including the 10 Phase 3 scenario subtests under `_vm-fleet-scenarios/`.

High-level categories:

- **Framework-level VMs** (`vm-core`, `vm-minimal`, `vm-infra`,
  `vm-nixfleet`, `vm-agent-rebuild`) — each one boots one or two nodes and
  exercises a single subsystem.
- **Fleet-level VMs** (`vm-fleet` and the 10 `vm-fleet-*` scenario
  subtests) — exercise multi-node topologies, mTLS, rollout strategies,
  failure paths, SSH-direct deploys, etc.

## Rust tests

Every Rust crate has unit tests in-file, and Phase 3 added ~60 integration
scenarios in `control-plane/tests/*_scenarios.rs` and `cli/tests/*_scenarios.rs`.
See [Rust Tests](rust-tests.md) for the full breakdown.

## Finding the right test for a symptom

| Symptom | Where to start |
|---|---|
| "Option X isn't being set correctly" | Eval test for that option |
| "My consumer flake doesn't build with mkHost" | `integration-mock-client` |
| "The agent service won't start on a real VM" | `vm-core`, `vm-nixfleet` |
| "A scope module (firewall, backup, monitoring) is broken" | `vm-infra` |
| "The fetch→apply pipeline isn't working" | `vm-agent-rebuild` |
| "Rollout state machine is wrong" | `vm-fleet` + Phase 3 Rust `failure_scenarios.rs`, `deploy_scenarios.rs` |
| "mTLS / auth / RBAC is wrong" | `vm-fleet-mtls-missing`, Rust `auth_scenarios.rs` |
| "Release CRUD or release push-hook is wrong" | `vm-fleet-release`, Rust `release_scenarios.rs` |
| "Bootstrap / admin-key flow is wrong" | `vm-fleet-bootstrap` |
| "SSH-direct deploy is broken" | `vm-fleet-deploy-ssh`, `vm-fleet-rollback-ssh` |
| "Tag sync from agent config isn't working" | `vm-fleet-tag-sync`, Rust `machine_scenarios.rs` |
| "Health check type X fails" | `vm-fleet-apply-failure`, agent `health::*` unit tests |
| "Rollout resume doesn't resume" | `vm-fleet-apply-failure`, Rust `failure_scenarios.rs` |
| "Metrics aren't being emitted" | Rust `metrics_scenarios.rs` |
| "Audit log is wrong / CSV injection" | Rust `audit_scenarios.rs` |

## Known coverage gaps

- **Real `switch-to-configuration`**: every VM test runs agents with `dryRun = true`
  so the actual apply path is not exercised in CI. Only production bootstraps
  cover it.
- **Agent-side `poll_hint` honouring**: the CP-side emission is covered by
  Rust scenarios P1 and P2, but the agent-side loop honouring the hint is
  deferred to a testability refactor (logged in `TODO.md`).
- **CLI env-var precedence**: the Rust scenario I2
  (`cli/tests/config_scenarios.rs`) is `#[ignore]` pending a Phase 4 fix for
  `config::resolve`.
- **Release delete CLI subcommand**: `nixfleet release delete` doesn't exist
  yet. `DELETE /api/v1/releases/{id}` is covered at the HTTP layer by R5/R6
  (Rust `release_scenarios.rs`) but there's no CLI dispatch test.
- **Multi-CP topologies** and **agenix secret rotation** have no tests at all.
