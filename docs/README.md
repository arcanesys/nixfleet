# nixfleet documentation

Map of what lives where. Every doc here is authoritative for its topic; when
the code disagrees, the code is being built to match.

## Design + contracts (read first)

| File | What it is | When to read |
|---|---|---|
| [`../ARCHITECTURE.md`](../ARCHITECTURE.md) | High-level architecture, component roles, trust flow, build order (Phases 0-10) | First read for new contributors |
| [`source-layout.md`](source-layout.md) | The four Nix layers (`lib/` / `modules/scopes/` / `contracts/` / `impls/`) and what goes where | When adding Nix code and unsure where it belongs |
| [`CONTRACTS.md`](CONTRACTS.md) | Every artifact, key, and format that crosses a stream boundary (data, trust roots, canonicalization, storage purity) | When adding or changing anything cross-stream |
| [`commercial-extensions.md`](commercial-extensions.md) | Capabilities deliberately out of scope for the open kernel (HA, SLA observability, audit packages) | When weighing whether a feature belongs in this repo |
| [`trust-root-flow.md`](trust-root-flow.md) | How `nixfleet.trust.*` declarations reach `verify_artifact` at runtime | When touching trust-root wiring |
| [`harness.md`](harness.md) | microvm.nix harness scope and slot-in points | When extending the integration-test fabric |

## Protocol + data (RFCs)

| File | Topic |
|---|---|
| [`rfcs/0001-fleet-nix.md`](./rfcs/0001-fleet-nix.md) | Declarative fleet shape: `mkFleet`, selectors, rollouts, edges, budgets, `fleet.resolved` artifact |
| [`rfcs/0002-reconciler.md`](./rfcs/0002-reconciler.md) | Reconciler state machine, decision procedure, verify path, failure handling |
| [`rfcs/0003-protocol.md`](./rfcs/0003-protocol.md) | Agent ↔ control-plane wire protocol, identity model, endpoints, security model |

## Operational reference (mdbook)

The [`mdbook/`](mdbook/) subtree is the user-facing manual. Source lives in
[`mdbook/src/`](mdbook/src/); the table of contents is
[`mdbook/src/SUMMARY.md`](mdbook/src/SUMMARY.md). Build with `nix run .#docs`.

| Section | Path |
|---|---|
| Introduction | [`mdbook/src/introduction.md`](mdbook/src/introduction.md) |
| Architecture overview | [`mdbook/src/architecture.md`](mdbook/src/architecture.md) |
| Operator cookbook | [`mdbook/src/operator-cookbook.md`](mdbook/src/operator-cookbook.md) |
| Troubleshooting | [`mdbook/src/troubleshooting.md`](mdbook/src/troubleshooting.md) |
| Rust API (generated) | [`mdbook/src/api.md`](mdbook/src/api.md) |
| Module options (generated) | [`mdbook/src/options.md`](mdbook/src/options.md) |

## Historical decisions

[`adr/`](adr/) - 11 Architecture Decision Records (001-011) covering `mkHost`,
flags-over-roles, agent-as-service-module, hydration, fire-and-forget apply, etc.

## Root-level docs

| File | What it is |
|---|---|
| [`../README.md`](../README.md) | User-facing README: install, quick start, ecosystem |
| [`../CHANGELOG.md`](../CHANGELOG.md) | Changelog (Keep a Changelog format) |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Contributor guide: setup, tests, commit conventions, license |
| [`../DISASTER-RECOVERY.md`](../DISASTER-RECOVERY.md) | Operator runbook for CP teardown + recovery |
| [`../SECURITY.md`](../SECURITY.md) | Security policy and disclosure |
| [`../CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | Code of conduct |
