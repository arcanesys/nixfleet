# impermanence

## Purpose

Ephemeral root filesystem with selective persistence. On every boot, the btrfs root subvolume is wiped and recreated. Only explicitly persisted paths survive reboots. This ensures the system stays clean and reproducible.

## Location

- `modules/scopes/impermanence.nix`

## Configuration

**Gate:** `isImpermanent`

### NixOS module (system-level persistence)

Persisted system directories:
- `/etc/nixos`
- `/etc/NetworkManager/system-connections`
- `/var/lib/systemd`
- `/var/lib/nixos`
- `/var/log`

Persisted system files:
- `/etc/machine-id`

### Btrfs root wipe (initrd)

Runs in `boot.initrd.postResumeCommands`:
1. Mount the btrfs root partition
2. Move `@root` to `old_roots/<timestamp>`
3. Delete old roots older than 30 days
4. Create fresh `@root` subvolume

### HM module (user-level persistence)

Persisted user directories: Documents, Downloads, Pictures, Videos, `.keys`, `.local/share/src`, `fleet`, `.zplug`, `.local/share/zsh`, `.config/gh`, `.local/share/nvim`, `.cache/nvim`, `.cache/tmux`, `.local/share/zoxide`, `.local/share/nix`.

Persisted user files: `.zsh_history`, `.ssh/known_hosts`.

### Activation script

Ensures `/persist/home/<user>` and `.keys` have correct ownership (user, not root).

## Scope-Aware Persist Paths

Other scopes should add their own persist paths when `isImpermanent` is true, rather than centralizing them here. For example:

```nix
# In a fleet scope module:
home.persistence."/persist".directories =
  lib.mkIf (osConfig.hostSpec.isImpermanent or false)
  [ ".config/my-app" ];
```

The NixFleet agent and control plane scopes also auto-persist their state directories when `isImpermanent` is true.

## Dependencies

- Input: `impermanence` (github:nix-community/impermanence)
- Depends on: hostSpec `isImpermanent` flag
- Darwin guard: HM module uses `lib.optionalAttrs (!hS.isDarwin)` (no impermanence on macOS)

## Links

- [Scope Overview](README.md)
- [Secrets bootstrap](../secrets/bootstrap.md)
