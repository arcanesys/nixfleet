# `securix-endpoint` — Sécurix endpoint under NixFleet

End-to-end example proving that [Sécurix](https://github.com/arcanesys/securix) (ANSSI-hardened NixOS distribution for government laptops) composes cleanly under [`nixfleet.lib.mkHost`](https://github.com/arcanesys/nixfleet).

## Composition

Three layers, each owning its concerns:

| Layer | Source | Role |
|---|---|---|
| Generic role | `nixfleet-scopes.scopes.roles.endpoint` | base CLI tools + secrets wiring + impermanence option surface |
| Distro modules | `securix.nixosModules.securix-base` + `securix.nixosModules.securix-hardware.t14g6` | ANSSI hardening, multi-operator users, VPN / PAM / audit, hardware profile for ThinkPad T14 Gen 6 |
| Host-specific tweaks | inline module in `flake.nix` | host identity, `securix.self` metadata, upstream workarounds |

NixFleet itself stays oblivious to ANSSI, Sécurix's SKU registry, lanzaboote, etc. It just composes NixOS modules.

## Build

```sh
# Pure evaluation (fast, no VM build):
nix flake check --no-build

# Full system closure (slow, builds everything):
nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel

# Deploy to fresh hardware:
nixos-anywhere --flake .#lab-endpoint root@<ip>
```

## Extending

To adapt this example for a real endpoint:

1. Replace `securix.self.user` / `machine` with real values (email, serial, inventory ID, hardware SKU from the list below).
2. Populate `hostSpec.sshAuthorizedKeys` with real operator keys.
3. Drop the workarounds (see below) once the consuming deploy has real disk / secrets / user data.
4. Consider adding an `isVm = true` for testing in QEMU before real hardware.

### Supported Sécurix hardware SKUs

- `x280`, `elitebook645g11`, `elitebook850g8`, `latitude5340`, `t14g6`, `x9-15`, `e14-g7`

Pick one that matches the target hardware — each SKU module enables hardware-specific kernel modules, power management, firmware, and so on.

## The 5 upstream workarounds

The inline module at the end of `flake.nix` sets a handful of options that would otherwise break eval because Sécurix's upstream modules expect them to be set by the consuming deploy (secrets, filesystems, agenix identities, etc.). The pilot example has none of those, so we stub them out. When moving to real hardware, drop each workaround as you add the real source of truth:

### 1. `_module.args = { operators = {}; vpnProfiles = {}; }`

Sécurix's `bastion` and `vpn` modules take `operators` and `vpnProfiles` as `_module.args` — normally supplied by a deploy-specific flake that enumerates real VPN profiles and per-operator credentials. For a bare pilot these are empty attrsets.

**Drop when:** the real deploy wires up operators + vpnProfiles via its own `_module.args` override.

### 2. `fileSystems."/" = { device = "/dev/vda1"; fsType = "ext4"; }`

Sécurix's `filesystems/` module sets up LUKS + btrfs + impermanence filesystems conditional on a real disk. The pilot gets a plain ext4 root so it evaluates without pulling disko/lanzaboote all the way through.

**Drop when:** the real deploy imports `disko` with a proper `disko.devices` configuration (see `nixfleet-scopes.scopes.disk-templates.btrfs-impermanence` for a starting point).

### 3. `boot.lanzaboote.enable = lib.mkOverride 0 false`

Sécurix enables lanzaboote by default for Secure Boot. The pilot disables it because Secure Boot keys are machine-specific and a generic eval can't provision them.

**Drop when:** you've generated Secure Boot keys (`sbctl create-keys`) and stored them at the path lanzaboote expects.

### 4. `boot.loader.systemd-boot.enable = lib.mkOverride 0 true`

Replaces the disabled lanzaboote bootloader with plain systemd-boot so the NixOS evaluation produces a bootable closure.

**Drop when:** re-enabling lanzaboote (see #3).

### 5. `users.allowNoPasswordLogin = true`

The pilot has no `hashedPasswordFile` for any user (keeping the example self-contained), so NixOS's normal "every user must have a password" assertion would fire. This flag relaxes it.

**Drop when:** supplying real `hashedPasswordFile` values (via agenix or otherwise) in `users.users.<name>`.

## Why this example exists

Documents the integration hypothesis between NixFleet and Sécurix — showing that two independently-designed frameworks can compose additively with minimal glue. The `modules = [...]` list is the full integration surface.

## See also

- [NixFleet](https://github.com/arcanesys/nixfleet) — `mkHost` and framework services
- [`nixfleet-scopes`](https://github.com/arcanesys/nixfleet-scopes) — generic roles and infrastructure scopes
- [Sécurix](https://github.com/arcanesys/securix) — ANSSI-hardened NixOS distribution
