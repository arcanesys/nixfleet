# Impermanence

Impermanent hosts wipe their root filesystem on every boot. Only explicitly persisted paths survive. This eliminates configuration drift and forces every piece of state to be declared.

## What ephemeral root gives you

- **No drift** - the root filesystem is always a clean slate. Undeclared state cannot accumulate.
- **Forced explicitness** - if you forget to persist something, you notice on the next reboot. No hidden state.
- **Reproducibility** - two machines with the same closure and the same persisted data behave identically.

## How the btrfs wipe works

On boot, an initrd script runs before the root filesystem is mounted:

1. Mounts the btrfs partition by label (`root`)
2. Renames the current `@root` subvolume to `old_roots/<timestamp>`
3. Deletes old root snapshots older than 30 days (recursive subvolume deletion)
4. Creates a fresh `@root` subvolume
5. Unmounts

The `/persist` filesystem is marked `neededForBoot = true` so it is available during early boot before the wipe completes.

## What the framework persists

### System-level (`/persist`)

| Path | Purpose |
|------|---------|
| `/etc/nixos` | NixOS configuration |
| `/etc/NetworkManager/system-connections` | WiFi/VPN connections |
| `/var/lib/systemd` | systemd state (timers, journals) |
| `/var/lib/nixos` | NixOS UID/GID maps |
| `/var/log` | System logs |
| `/etc/machine-id` | Stable machine identity (file) |

### User-level (`/persist` via Home Manager)

The framework persists common user paths. Fleet repos extend this list with their own application state via scope-aware persistence (see below).

| Path | Purpose |
|------|---------|
| `.keys` | Encryption/decryption keys |
| `.local/share/nix` | Nix user state |
| `.ssh/known_hosts` | SSH known hosts (file) |

The framework also persists paths for tools included in the base scope (shell history, plugin state, CLI auth). See `modules/scopes/_impermanence.nix` for the full list.

User-level mounts are hidden (`hideMounts = true`) to keep `ls` output clean.

### Service-level (auto-persist)

The agent and control plane modules automatically persist their state directories when impermanence is enabled:

- **Agent**: `/var/lib/nixfleet` (SQLite state database)
- **Control plane**: `/var/lib/nixfleet-cp` (SQLite state database)

No manual configuration needed. The service modules detect `nixfleet.impermanence.enable` and add the persist entries.

## Scope-aware persistence

Persist paths belong next to the program they support, not in a centralized list. When you write a scope that installs a program with state, co-locate the persist declaration:

```nix
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isGraphical {
    programs.firefox.enable = true;

    # Persist Firefox profile alongside its config
    home.persistence."/persist" = lib.mkIf config.nixfleet.impermanence.enable {
      directories = [".mozilla/firefox"];
    };
  };
}
```

This prevents the persistence list from drifting out of sync with installed programs.

## Opting in

Enable `nixfleet.impermanence.enable` (or use a role that sets it) and use a btrfs disk layout with separate persist subvolumes:

```nix
nixfleet.lib.mkHost {
  hostName = "myhost";
  platform = "x86_64-linux";
  hostSpec = {
    userName = "alice";
  };
  modules = [
    nixfleet-scopes.scopes.roles.workstation
    { nixfleet.impermanence.enable = true; }
    # Use the framework's btrfs-impermanence disko template
    nixfleet.diskoTemplates.btrfs-impermanence
    ./hardware-configuration.nix
  ];
}
```

The framework provides two disko templates:

- `diskoTemplates.btrfs` - standard btrfs layout without impermanence
- `diskoTemplates.btrfs-impermanence` - btrfs layout with `@root`, `@persist`, and `@nix` subvolumes

## Ownership and activation

The framework runs an activation script that ensures `/persist/home/<userName>` exists with correct ownership. If a `.keys` directory exists in the persist home, it is recursively chowned to the primary user.
