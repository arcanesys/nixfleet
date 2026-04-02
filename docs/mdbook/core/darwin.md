# core/darwin.nix

## Purpose

Universal Darwin (macOS) configuration. Nix settings compatible with Determinate installer, TouchID sudo, declarative dock management, system defaults, and agenix secrets.

## Location

- `modules/core/darwin.nix`

## Configuration

### Nix settings
- `nix.enable = false` (Determinate installer manages the nix daemon)
- No automatic gc (incompatible with `nix.enable = false`)
- Manual gc: `nix store gc --max 20G`
- Binary caches: nixos + nix-community cachix

### TouchID sudo
- `security.pam.services.sudo_local.touchIdAuth = true`
- pam-reattach for SSH session support

### Users
- Primary user with zsh shell
- Home at `/Users/<userName>`

### Secrets (agenix -- fleet-level)
- Agenix configuration lives in fleet-level modules (injected via `mkHost` modules), not in `core/darwin.nix`
- Identity: `~/.ssh/id_ed25519`
- Secrets: `github-ssh-key` (symlink), `github-signing-key` (copy)
- Group: `staff` (macOS convention)

### System defaults
- **Keyboard:** fast repeat (2), short initial delay (15), press-and-hold disabled
- **Dock:** autohide, no recents, bottom, 48px tiles
- **Finder:** show all extensions + hidden files, path bar, sort folders first
- **Trackpad:** tap-to-click, three-finger drag

### Dock management
Activation script compares current dock entries with desired entries (URI-based diff). Only resets dock when changed. Uses `dockutil` for add/remove operations.

## Dependencies

- `hostSpec.isDarwin` set to `true`
- Agenix is a fleet-level concern, injected via `mkHost` modules

## Links

- [Core Overview](README.md)
