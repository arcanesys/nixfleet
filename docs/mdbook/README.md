# NixFleet Documentation

Declarative NixOS fleet management -- hosts defined as code with `mkHost`, self-activating scopes, and standard deployment tools.

## [Guide](guide/README.md)

Getting started, concepts, development workflow, and advanced topics. Start here if you are new to NixFleet.

## [Reference](architecture.md)

Technical reference for hosts, scopes, core modules, apps, testing, and secrets management.

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

# Format all Nix files
nix fmt
```
