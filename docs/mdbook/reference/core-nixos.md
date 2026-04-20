# Core NixOS Module

Everything configured by `_nixos.nix`, imported automatically by mkHost for Linux platforms.

## Nixpkgs

| Setting | Value |
|---------|-------|
| `allowUnfree` | `true` |
| `allowBroken` | `false` |
| `allowInsecure` | `false` |
| `allowUnsupportedSystem` | `true` |

## Nix settings

| Setting | Value |
|---------|-------|
| `nixPath` | `[]` (mkDefault) |
| `allowed-users` | `[<userName>]` |
| `trusted-users` | `["@admin"]` + `<userName>` (unless the server role is active) |
| `substituters` | `["https://nix-community.cachix.org" "https://cache.nixos.org"]` |
| `trusted-public-keys` | nix-community + cache.nixos.org keys |
| `auto-optimise-store` | `true` |
| `experimental-features` | `nix-command flakes` |
| `gc.automatic` | `true` |
| `gc.dates` | `weekly` |
| `gc.options` | `--delete-older-than 7d` |

## Boot

| Setting | Value |
|---------|-------|
| `loader.systemd-boot.enable` | `true` |
| `loader.systemd-boot.configurationLimit` | `42` |
| `loader.efi.canTouchEfiVariables` | `true` |
| `initrd.availableKernelModules` | `xhci_pci`, `ahci`, `nvme`, `usbhid`, `usb_storage`, `sd_mod` |
| `kernelPackages` | `linuxPackages_latest` |
| `kernelModules` | `["uinput"]` |

## Localization

| Setting | Source |
|---------|--------|
| `time.timeZone` | `hostSpec.timeZone` |
| `i18n.defaultLocale` | `hostSpec.locale` |
| `console.keyMap` | `hostSpec.keyboardLayout` (mkDefault) |

## Networking

| Setting | Value |
|---------|-------|
| `hostName` | `hostSpec.hostName` |
| `useDHCP` | `false` |
| `networkmanager.enable` | `true` |
| `firewall.enable` | `true` |
| Interface DHCP | Enabled for `hostSpec.networking.interface` when set |

## Programs

| Program | Setting |
|---------|---------|
| `gnupg.agent` | Enabled with SSH support |
| `dconf` | Enabled |
| `git` | Enabled |
| `zsh` | Enabled, completion disabled (managed by HM) |

## Security

| Setting | Value |
|---------|-------|
| `polkit.enable` | `true` |
| `sudo.enable` | `true` |
| Sudo NOPASSWD | `reboot` for `wheel` group |

## Users

### Primary user (`hostSpec.userName`)

| Setting | Value |
|---------|-------|
| `isNormalUser` | `true` |
| `extraGroups` | `wheel` + `audio`, `video`, `docker`, `git`, `networkmanager` (if groups exist) |
| `shell` | `zsh` |
| `openssh.authorizedKeys.keys` | `hostSpec.sshAuthorizedKeys` |
| `hashedPasswordFile` | `hostSpec.hashedPasswordFile` (when non-null) |

### Root

| Setting | Value |
|---------|-------|
| `openssh.authorizedKeys.keys` | `hostSpec.sshAuthorizedKeys` |
| `hashedPasswordFile` | `hostSpec.rootHashedPasswordFile` (when non-null) |

## SSH hardening

| Setting | Value |
|---------|-------|
| `services.openssh.enable` | `true` |
| `PermitRootLogin` | `prohibit-password` |
| `PasswordAuthentication` | `false` |
| `KbdInteractiveAuthentication` | `false` |

## Other services

| Setting | Value |
|---------|-------|
| `services.printing.enable` | `false` |
| `services.xserver.xkb.layout` | `hostSpec.keyboardLayout` (mkDefault) |
| `hardware.ledger.enable` | `true` |

## System packages

- `git`
- `inetutils`

## State version

`system.stateVersion` = `"24.11"` (mkDefault)
