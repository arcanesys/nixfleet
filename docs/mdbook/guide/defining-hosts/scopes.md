# Scopes

Scopes are plain NixOS and Home Manager modules that self-activate based on hostSpec flags. mkHost always includes them, but they produce no configuration when their activation condition is false -- Nix's lazy evaluation means zero overhead for inactive scopes.

## How scopes work

A scope is a set of NixOS and/or HM modules grouped by concern. Each scope wraps its `config` block in `lib.mkIf` on a hostSpec flag:

```nix
# Simplified pattern
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isImpermanent {
    # Only evaluated when isImpermanent = true
    environment.persistence."/persist/system" = { ... };
  };
}
```

Scopes return module attrsets (e.g. `{ nixos, darwin, homeManager }`) that mkHost imports into the appropriate evaluation contexts.

## Framework scopes

These ship with NixFleet and are auto-included by mkHost.

| Scope | Activation | Provides |
|-------|-----------|----------|
| **base** | `!isMinimal` | CLI packages (coreutils, ripgrep, fd, fzf, jq, eza, gh, nh, etc.) via HM. Linux-only system packages (ifconfig, netstat, xdg-utils) via NixOS. Darwin-only packages (dockutil, mas) via Darwin module. |
| **impermanence** | `isImpermanent` | Btrfs root wipe on boot (initrd script), system-level persist paths (`/etc/nixos`, `/var/lib/systemd`, `/var/log`, etc.), user-level persist paths (`.keys`, shell state, editor state, SSH known_hosts). Linux only. |
| **nixfleet-agent** | `services.nixfleet-agent.enable` | Systemd service for the fleet management agent. Polls the control plane, applies deployments, reports health. Hardened service config. |
| **nixfleet-control-plane** | `services.nixfleet-control-plane.enable` | Systemd service for the fleet control plane HTTP server. Optional firewall opening. `GET /metrics` always available; routes split: agent-facing (mTLS, no API key) vs admin (API key required). |
| **firewall** | `!isMinimal` | Enables nftables, SSH rate limiting (5 connections/min), and drop logging. Auto-activates on all non-minimal hosts. |
| **secrets** | `nixfleet.secrets.enable` | Computes `resolvedIdentityPaths` from hostSpec (host key primary, user key fallback on workstations). Handles impermanence persistence and boot ordering. Fleet repos wire the actual backend (agenix, sops-nix). |
| **backup** | `nixfleet.backup.enable` | Systemd timer, pre/post hooks, health check pings, and status reporting. Fleet repos supply the actual backup command (restic, borgbackup, etc.). |
| **monitoring** | `nixfleet.monitoring.nodeExporter.enable` | Prometheus node exporter with fleet-tuned collector defaults. |

The agent and control-plane scopes are NixOS service modules (not hostSpec-gated). They follow the standard NixOS `enable` pattern.

The framework ships two kinds of scopes: **automatic** (base, impermanence, firewall — activated by hostSpec flags) and **opt-in** (secrets, backup, monitoring, agent, control-plane — require explicit enable). Automatic scopes provide universal infrastructure. Opt-in scopes let fleet repos choose what they need.

## Scope-aware persistence

Persist paths live alongside the program definitions they belong to, not in a centralized list. The impermanence scope defines only universal persist paths. Other scopes (including fleet-defined ones) add their own paths where they are declared:

```nix
# A fleet-level scope that adds Firefox
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isGraphical {
    programs.firefox.enable = true;

    # Persist Firefox profile alongside the Firefox config
    home.persistence."/persist" = lib.mkIf hS.isImpermanent {
      directories = [".mozilla/firefox"];
    };
  };
}
```

This keeps persistence declarations co-located with the programs they support, rather than maintaining a separate list that drifts out of sync.

## Fleet-defined scopes

The framework provides only generic infrastructure scopes. Opinionated scopes belong in your fleet repo. Common patterns:

| Scope | Flag | Provides |
|-------|------|----------|
| `dev` | `isDev` | Docker, build tools, language runtimes |
| `graphical` | `isGraphical` | Audio (PipeWire), fonts, display manager |
| `desktop` | `useDesktop` | Window manager, status bar, screen sharing |

These are examples. Your fleet defines whatever scopes make sense for your organization.

## Writing a scope

A scope is just a Nix file that returns module attrsets:

```nix
# modules/scopes/dev.nix (in your fleet repo)
{
  nixos = {config, lib, pkgs, ...}: let
    hS = config.hostSpec;
  in {
    config = lib.mkIf hS.isDev {
      virtualisation.docker.enable = true;
      virtualisation.docker.storageDriver = "btrfs";
    };
  };

  homeManager = {config, lib, pkgs, ...}: let
    hS = config.hostSpec;
  in {
    home.packages = lib.optionals hS.isDev (with pkgs; [
      gcc
      gnumake
      nodejs
    ]);
  };
}
```

Include it in your mkHost calls by importing the scope's nixos and homeManager modules in the appropriate places, or use an import-tree pattern to auto-discover scope files.

## What belongs where

| Content | Location |
|---------|----------|
| Universal CLI tools, basic system config | Framework `base` scope |
| Ephemeral root, core persist paths | Framework `impermanence` scope |
| Fleet agent/control-plane services | Framework service modules |
| Dev tools, graphical stack, theming, dotfiles | Fleet-level scopes |
| Hardware-specific config | Per-host modules |
