# Testing Overview

nixfleet has four test tiers that together cover configuration, Rust code,
Nix module wiring, and full multi-node runtime behaviour. There is exactly
**one command that runs everything**:

```sh
nix run .#validate -- --all
```

That's it. Use this for CI, for pre-merge, for pre-release, and for "did my
change break something far from where I was editing". When you need a smaller
slice for an inner-loop iteration, the flag variants below trade coverage
for speed:

| Command | Runs | Typical duration |
|---|---|---|
| `nix run .#validate` | format + `nix flake check` + eval-* + host builds | ~1 min |
| `nix run .#validate -- --vm` | ^ + every `vm-*` check (dynamically discovered) | ~20–40 min |
| `nix run .#validate -- --rust` | ^ + `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + nix-build of every Rust package (sandboxed test run) | ~5–8 min |
| `nix run .#validate -- --all` | Everything | ~25–45 min |

What `--all` actually runs, in order:

1. **Formatting** — `nix fmt --fail-on-change`
2. **Flake eval** — `nix flake check --no-build` (every flake output type-checks)
3. **Eval tests** — all `eval-*` derivations under `.#checks`
4. **Host builds** — every `nixosConfigurations.<host>.config.system.build.toplevel`
5. **VM tests** — every `vm-*` under `.#checks`, discovered dynamically
6. **Rust workspace tests** — `cargo test --workspace` in the dev shell
7. **Rust lints** — `cargo clippy --workspace --all-targets -- -D warnings`
8. **Rust package builds** — `nix build .#packages.<system>.{nixfleet-agent,nixfleet-control-plane,nixfleet-cli}` (runs `cargo test` inside the nix sandbox — catches environment-dependent test failures that the dev-shell `cargo test` misses)

### Inner-loop iteration (drilling down when something fails)

When `--all` surfaces a failure, you can reproduce the failing tier with a
narrower command. Prefer these only after `--all` has already failed:

```sh
# Single VM scenario
nix build .#checks.x86_64-linux.vm-fleet-apply-failure --no-link

# Single Rust test binary
nix develop --command cargo test -p nixfleet-control-plane --test route_coverage

# Single test function
nix develop --command cargo test -p nixfleet-agent --test run_loop_scenarios \
    poll_hint_shortens_next_interval
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

- **Real `switch-to-configuration`**: most VM tests run agents with `dryRun = true`
  so the actual apply path is not exercised. The exception is
  `vm-agent-rebuild`, which runs with `dryRun = false` and exercises the
  missing-path guard end-to-end. Production bootstraps cover the happy
  apply path.
- **Multi-CP topologies** and **agenix secret rotation** have no tests.
