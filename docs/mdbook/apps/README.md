# Apps

## Purpose

Flake apps defined in `modules/apps.nix` as `perSystem` shell scripts. Provide VM management and validation tooling. Deployment uses standard NixOS commands directly (nixos-anywhere, nixos-rebuild, darwin-rebuild).

## Location

- `modules/apps.nix` — framework apps (validate, VM helpers)
- `modules/_shared/lib/mk-vm-apps.nix` — VM helper generator for fleet repos

## App Table

| App | Command | Platform | Description |
|-----|---------|----------|-------------|
| [validate](validate.md) | `nix run .#validate` | All | Full validation suite (format + eval + host builds) |
| [spawn-qemu](spawn-qemu.md) | `nix run .#spawn-qemu` | Linux | QEMU VM launcher (includes `--persistent` mode) |
| [test-vm](test-vm.md) | `nix run .#test-vm` | Linux | Automated ISO-to-verify cycle |
| [spawn-utm](spawn-utm.md) | `nix run .#spawn-utm` | Darwin | UTM VM setup guide |

## Deployment Commands (Standard Tooling)

```sh
nixos-anywhere --flake .#hostname root@ip              # fresh install
sudo nixos-rebuild switch --flake .#hostname            # local rebuild
nixos-rebuild switch --flake .#hostname --target-host root@ip  # remote rebuild
darwin-rebuild switch --flake .#hostname                # macOS
```

## mkVmApps for Fleet Repos

Fleet repos wire VM helpers into their own apps output:

```nix
perSystem = {pkgs, ...}: {
  apps = inputs.nixfleet.lib.mkVmApps {inherit pkgs;};
};
```

## DevShell

`apps.nix` also defines the default devShell (`nix develop`) with:
- `bashInteractive`, `git`, `age`
- shellHook: sets `EDITOR=vim` and activates git hooks (`.githooks/`)
