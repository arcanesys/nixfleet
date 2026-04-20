# Cross-Platform

NixFleet supports NixOS and macOS from a single API. mkHost detects the platform from the `platform` parameter and builds the appropriate system type.

## Supported platforms

| Platform | System builder | Init system | Notes |
|----------|---------------|-------------|-------|
| `x86_64-linux` | `nixosSystem` | systemd | Full feature set |
| `aarch64-linux` | `nixosSystem` | systemd | Full feature set (ARM servers, edge devices) |
| `aarch64-darwin` | `darwinSystem` | launchd | Apple Silicon Macs |
| `x86_64-darwin` | `darwinSystem` | launchd | Intel Macs |

## Automatic platform detection

mkHost sets `hostSpec.isDarwin` based on the `platform` parameter. You never set it manually. The `home` option also auto-computes:

- Linux: `/home/<userName>`
- Darwin: `/Users/<userName>`

## What differs by platform

| Concern | NixOS | Darwin |
|---------|-------|--------|
| Core module | `_nixos.nix` - boot, systemd-boot, NetworkManager, polkit, SSH | `_darwin.nix` - system defaults, TouchID sudo, dock management |
| User config | `users.users.<name>.isNormalUser` | `users.users.<name>.home`, `.isHidden` |
| Services | systemd services (`systemd.services.*`) | launchd agents (`launchd.agents.*`) |
| Impermanence | Btrfs root wipe, `/persist` bind mounts | Not applicable |
| Base scope packages | ifconfig, netstat, xdg-utils (system) | dockutil, mas (system) |
| Home Manager | HM NixOS module + impermanence HM module | HM Darwin module (no impermanence) |
| Nix daemon | Managed by NixOS (`nix.gc.automatic`, etc.) | Determinate-compatible (`nix.enable = false`) |
| Trusted users | `@admin` + user (non-server) | `@admin` + user |

## Platform guards in modules

Use `hostSpec.isDarwin` (or `pkgs.stdenv`) for platform-specific logic:

```nix
# Using hostSpec (available in all mkHost modules)
{config, lib, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf (!hS.isDarwin) {
    # Linux-only configuration
    services.openssh.enable = true;
  };
}
```

```nix
# Using stdenv (standard Nix pattern)
{lib, pkgs, ...}: {
  home.packages = lib.optionals pkgs.stdenv.isLinux [pkgs.strace]
    ++ lib.optionals pkgs.stdenv.isDarwin [pkgs.darwin.apple_sdk.frameworks.Security];
}
```

Both approaches work. `hostSpec.isDarwin` is preferred in NixFleet modules because it is available without `pkgs` and is consistent with the hostSpec-driven activation pattern.

## Scopes and platform support

Not all framework scopes apply to both platforms:

| Scope | NixOS | Darwin |
|-------|-------|--------|
| base | NixOS module + HM module | Darwin module + HM module |
| impermanence | NixOS module + HM module | Not included |
| nixfleet-agent | NixOS service (systemd) | Not available |
| nixfleet-control-plane | NixOS service (systemd) | Not available |

The agent and control-plane services are NixOS-only (systemd). macOS hosts are managed through standard `darwin-rebuild` and do not participate in fleet orchestration.

## Design principle

Prefer simple platform-specific implementations over complex cross-platform abstractions. If a feature only makes sense on one platform, keep it there. The framework handles the platform split at the mkHost level - individual modules should stay focused on their target platform rather than adding conditionals for every difference.

## Mixed fleet example

```nix
let
  org = {
    userName = "ops";
    timeZone = "UTC";
    sshAuthorizedKeys = ["ssh-ed25519 AAAA..."];
  };
in {
  # NixOS server
  web-01 = mkHost {
    hostName = "web-01";
    platform = "x86_64-linux";
    hostSpec = org;
    modules = [nixfleet-scopes.scopes.roles.server ./hosts/web-01/hardware.nix];
  };

  # macOS developer laptop
  dev-mac = mkHost {
    hostName = "dev-mac";
    platform = "aarch64-darwin";
    hostSpec = org;
    modules = [./hosts/dev-mac/extras.nix];
  };

  # ARM edge device
  sensor-01 = mkHost {
    hostName = "sensor-01";
    platform = "aarch64-linux";
    hostSpec = org;
    modules = [nixfleet-scopes.scopes.roles.endpoint ./hosts/sensor/hardware.nix];
  };
}
```

All three hosts share org defaults and use the same `mkHost` call. The framework selects the right system builder, core module, and scope set based on `platform`.
