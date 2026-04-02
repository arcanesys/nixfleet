# NixFleet User Guide

Manage your NixOS fleet declaratively -- hosts defined as code with `mkHost`, shared defaults, and self-activating scopes.

## What is NixFleet?

A framework for managing fleets of NixOS and macOS machines. Define your hosts with `nixfleet.lib.mkHost`, set hostSpec flags, and deploy with standard NixOS tools -- reproducible builds, instant rollback, zero config drift.

## Key Features

- **Single API** -- `mkHost` returns standard `nixosSystem`/`darwinSystem`
- **Scope-based architecture** -- features self-activate based on host flags
- **Impermanent root** -- ephemeral filesystem, only persist what matters
- **Secrets-agnostic** -- extension points for agenix, sops-nix, Vault, or any tool
- **Automated testing** -- eval tests, VM tests, one-command validation
- **Cross-platform** -- same framework drives NixOS and macOS
- **Standard deployment** -- nixos-anywhere, nixos-rebuild, darwin-rebuild

## How to Read This Guide

- **New to NixOS?** Start with [Why NixOS?](concepts/why-nixos.md) then [Quick Start](getting-started/quick-start.md)
- **Setting up your fleet?** Go to [Installation](getting-started/installation.md)
- **Day-to-day fleet ops?** See [Daily Usage](getting-started/daily-usage.md)
- **Adding hosts or scopes?** Read [Adding a New Host](advanced/new-host.md), [New Scope](advanced/new-scope.md)

## Quick Commands

```sh
# Rebuild NixOS after changes
sudo nixos-rebuild switch --flake .#<hostname>

# Rebuild macOS after changes
darwin-rebuild switch --flake .#<hostname>

# Fresh install on remote machine
nixos-anywhere --flake .#<hostname> root@<ip>

# Run all validations
nix run .#validate
```
