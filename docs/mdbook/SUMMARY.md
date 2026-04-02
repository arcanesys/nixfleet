# Summary

[Introduction](README.md)

---

# Guide

- [Overview](guide/README.md)

## Getting Started

- [Quick Start](guide/getting-started/quick-start.md)
- [Installation](guide/getting-started/installation.md)
- [Day-to-Day Usage](guide/getting-started/daily-usage.md)

## Concepts

- [Why NixOS?](guide/concepts/why-nixos.md)
- [Declarative Configuration](guide/concepts/declarative.md)
- [The Scope System](guide/concepts/scopes.md)
- [Impermanence](guide/concepts/impermanence.md)
- [Secrets Management](guide/concepts/secrets.md)

## Development

- [Testing Your Config](guide/development/testing.md)
- [VM Testing](guide/development/vm-testing.md)
- [GitHub Workflow](guide/development/github-workflow.md)

## Advanced

- [Adding a New Host](guide/advanced/new-host.md)
- [Adding a New Scope](guide/advanced/new-scope.md)
- [Cross-Platform Design](guide/advanced/cross-platform.md)
- [Security Model](guide/advanced/security.md)

---

# Reference

- [Architecture](architecture.md)
- [CLI Reference](cli/README.md)
- [Hosts](hosts/README.md)
- [Scopes](scopes/README.md)
  - [base](scopes/base.md)
  - [impermanence](scopes/impermanence.md)
  - [NixFleet Agent](scopes/nixfleet-agent.md)
  - [NixFleet Control Plane](scopes/nixfleet-control-plane.md)
- [Core Modules](core/README.md)
  - [nixos](core/nixos.md)
  - [darwin](core/darwin.md)
- [Apps](apps/README.md)
  - [validate](apps/validate.md)
  - [spawn-qemu](apps/spawn-qemu.md)
  - [spawn-utm](apps/spawn-utm.md)
  - [test-vm](apps/test-vm.md)
- [Testing](testing/README.md)
  - [Eval Tests](testing/eval-tests.md)
  - [VM Tests](testing/vm-tests.md)
- [Secrets](secrets/README.md)
  - [Bootstrap](secrets/bootstrap.md)
  - [WiFi](secrets/wifi.md)
