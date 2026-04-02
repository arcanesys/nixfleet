# core/nixos.nix

## Purpose

Universal NixOS configuration applied to every NixOS host. Provides Nix settings, boot, networking, user management, SSH hardening, and security.

## Location

- `modules/core/nixos.nix`

## Configuration

### Nix settings
- `allowUnfree = true`, `allowBroken = false`, `allowInsecure = false`
- Binary caches: `cache.nixos.org` + `nix-community.cachix.org`
- `auto-optimise-store = true` (hardlink deduplication)
- Weekly GC (`--delete-older-than 7d`)
- `trusted-users` includes regular user (except on servers)
- `experimental-features = nix-command flakes`

### Boot
- systemd-boot with 42-configuration limit
- Latest kernel (`linuxPackages_latest`)
- initrd modules: xhci_pci, ahci, nvme, usbhid, usb_storage, sd_mod
- uinput kernel module

### Localization
- `time.timeZone` from `hostSpec.timeZone`
- `i18n.defaultLocale` from `hostSpec.locale`
- `console.keyMap` from `hostSpec.keyboardLayout`

### Networking
- `hostName` from `hostSpec.hostName`
- NetworkManager enabled
- Firewall enabled
- Per-interface DHCP when `hostSpec.networking.interface` is set

### Programs
- gnupg agent with SSH support
- dconf, git, zsh (system-level)

### Security
- polkit enabled
- sudo with NOPASSWD reboot for wheel group

### Users
- Primary user: normal user, wheel + optional groups (audio, video, docker, git, networkmanager)
- Shell: zsh
- SSH authorized keys from `hostSpec.sshAuthorizedKeys`
- Hashed password from `hostSpec.hashedPasswordFile` / `hostSpec.rootHashedPasswordFile` (null = no managed password)

### SSH hardening
- `PermitRootLogin = "prohibit-password"`
- `PasswordAuthentication = false`
- `KbdInteractiveAuthentication = false`

### Hardware
- `hardware.ledger.enable = true`

### System packages
`git`, `inetutils`

## Dependencies

- Inputs: disko (imported for disk partitioning)
- Agenix and secrets are fleet-level concerns injected via `mkHost` modules -- not in this module

## Links

- [Core Overview](README.md)
