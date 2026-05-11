# NixFleet documentation

The doc tree splits by audience and lifecycle.

## Design (read first for contributors)

| File | Topic |
|------|-------|
| [design/architecture.md](design/architecture.md) | Components, trust flow, scaling envelope, where v0.2 lands |
| [design/contracts.md](design/contracts.md) | Every artefact, key, and format that crosses a stream boundary |
| [design/source-layout.md](design/source-layout.md) | The four Nix layers + dependency-pinning policy |

## Reference

| File | Topic |
|------|-------|
| [reference/harness.md](reference/harness.md) | microvm.nix integration-test fabric |
| [reference/crates/](reference/crates/index.md) | One condensed overview per Rust crate, linking to rustdoc |

## Operations

| File | Topic |
|------|-------|
| [operations/disaster-recovery.md](operations/disaster-recovery.md) | CP teardown + recovery runbook |
| [operations/operator-cookbook.md](operations/operator-cookbook.md) | Tasks the operator does, with concrete commands |
| [operations/troubleshooting.md](operations/troubleshooting.md) | Known failure modes from real-hardware testing |

## RFCs

See [rfcs/index.md](rfcs/index.md). The v0.2 protocol contract is owned by RFC-0001/0002/0003; the v0.3 trajectory by RFC-0004/0005/0006/0007.

## Composed view (mdbook)

The same content as a browseable book: `nix run .#docs` builds it, `nix run .#docs-serve` opens it locally. Configuration in [mdbook/book.toml](mdbook/book.toml); table of contents in [mdbook/src/SUMMARY.md](mdbook/src/SUMMARY.md). The wrapper files under `mdbook/src/{design,reference,operations,rfcs}/` are 1-line `{{#include}}` shims that pull from the canonical sources above - no duplication.

## Top-level meta-files

| File | What it is |
|------|-----------|
| [../README.md](../README.md) | User-facing project README |
| [../CHANGELOG.md](../CHANGELOG.md) | Release notes |
| [../CONTRIBUTING.md](../CONTRIBUTING.md) | Contributor guide |
| [../SECURITY.md](../SECURITY.md) | Security policy and disclosure |
| [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) | Code of conduct |
