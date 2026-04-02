# Why NixOS?

The motivation behind using NixOS and Nix for system configuration.

## The Problem

Traditional system configuration is fragile:
- Install a package, tweak a config, forget what you changed
- Rebuilding a machine from scratch takes hours of manual setup
- "Works on my machine" is the norm, not the exception
- Dotfile managers help, but only cover user-level configuration

## Nix's Answer

Nix treats system configuration like source code:

- **Declarative** — describe what you want, not how to get there
- **Reproducible** — same inputs always produce the same outputs
- **Atomic** — upgrades either succeed completely or don't happen
- **Rollbackable** — every generation is preserved, switch back instantly

## What This Config Achieves

This repository is a single source of truth for:

- **System-level config** — boot, networking, security, services
- **User-level config** — shell, editor, git, terminal, desktop
- **Secrets** — SSH keys, passwords, WiFi credentials (encrypted)
- **Disk layout** — partitioning is declarative (disko)
- **Ephemeral root** — the filesystem wipes on boot, only persisting what matters

One `sudo nixos-rebuild switch --flake .#hostname` and your entire system matches the repo. Change a flag, rebuild, and features appear or disappear.

## Trade-offs

Nix is not without cost:

- **Learning curve** — the Nix language and module system take time
- **Build times** — first builds are slow (cached afterward)
- **Debugging** — error messages can be cryptic
- **Ecosystem gaps** — not every tool has a Nix module

For this config, the benefits far outweigh the costs. Multiple machines stay in sync, reinstalls take minutes, and the entire system is version-controlled.

## Further Reading

- [Declarative Configuration](declarative.md) — how the module system works
- [The Scope System](scopes.md) — feature organization
- [Technical Architecture](../../architecture.md) — detailed module structure
