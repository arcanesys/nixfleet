# Core Darwin Module

Everything configured by `_darwin.nix`, imported automatically by mkHost for Darwin platforms.

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
| `nix.enable` | `false` (Determinate installer compatible) |
| `trusted-users` | `["@admin" "<userName>"]` |
| `substituters` | `["https://nix-community.cachix.org" "https://cache.nixos.org"]` |
| `trusted-public-keys` | nix-community + cache.nixos.org keys |
| `auto-optimise-store` | `true` |
| `experimental-features` | `nix-command flakes` |

## Programs

| Program | Setting |
|---------|---------|
| `zsh` | Enabled, completion disabled (managed by HM) |

## Users

| Setting | Value |
|---------|-------|
| `users.users.<userName>.name` | `<userName>` |
| `users.users.<userName>.home` | `hostSpec.home` |
| `users.users.<userName>.isHidden` | `false` |
| `users.users.<userName>.shell` | `zsh` |

## TouchID sudo

| Setting | Value |
|---------|-------|
| `security.pam.services.sudo_local.touchIdAuth` | `true` |
| PAM config | `pam_reattach.so` (ignore_ssh) + `pam_tid.so` |

TouchID works for sudo in terminal sessions, including through tmux via `pam_reattach`.

## System defaults

### NSGlobalDomain

| Key | Value |
|-----|-------|
| `AppleShowAllExtensions` | `true` |
| `ApplePressAndHoldEnabled` | `false` |
| `KeyRepeat` | `2` |
| `InitialKeyRepeat` | `15` |
| `com.apple.mouse.tapBehavior` | `1` |
| `com.apple.sound.beep.feedback` | `0` |

### Dock

| Key | Value |
|-----|-------|
| `autohide` | `true` |
| `show-recents` | `false` |
| `launchanim` | `true` |
| `orientation` | `bottom` |
| `tilesize` | `48` |

### Finder

| Key | Value |
|-----|-------|
| `AppleShowAllExtensions` | `true` |
| `AppleShowAllFiles` | `true` |
| `ShowPathbar` | `true` |
| `_FXSortFoldersFirst` | `true` |
| `_FXShowPosixPathInTitle` | `false` |

### Trackpad

| Key | Value |
|-----|-------|
| `Clicking` | `true` |
| `TrackpadThreeFingerDrag` | `true` |

## Dock management

The module includes a `local.dock` option for declarative Dock management using `dockutil`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `local.dock.enable` | `bool` | `true` | Enable dock management |
| `local.dock.entries` | `listOf submodule` | -- (readOnly) | Dock entries |

Each entry has:

| Sub-option | Type | Default |
|------------|------|---------|
| `path` | `str` | -- |
| `section` | `str` | `"apps"` |
| `options` | `str` | `""` |

The activation script diffs current Dock state against the declared entries and only resets when they differ.

## Other

| Setting | Value |
|---------|-------|
| `system.stateVersion` | `4` |
| `system.checks.verifyNixPath` | `false` |
| `system.primaryUser` | `<userName>` |
| `hostSpec.isDarwin` | `true` |
