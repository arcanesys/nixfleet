# base

## Purpose

Universal packages for all non-minimal hosts. Provides core CLI tools, file utilities, and Nix system management. Split across NixOS system packages, Darwin system packages, and HM user packages.

## Location

- `modules/scopes/base.nix`

## Configuration

**Gate:** `!isMinimal`

### NixOS system packages
`ifconfig`, `netstat`, `xdg-utils`

### Darwin system packages
`dockutil`, `mas` (Mac App Store CLI)

### HM user packages
- **Core CLI:** coreutils, killall, openssh, wget, age, gnupg, fastfetch, gh
- **File tools:** duf, eza, fd, fzf, jq, procs, ripgrep, tldr, tree, yq
- **Nix management:** home-manager, nh

## Dependencies

- Depends on: hostSpec `isMinimal` flag
- Depended on by: all non-minimal hosts

## Links

- [Scope Overview](README.md)
