# `securix-endpoint` — Sécurix endpoint under NixFleet

End-to-end example proving that [Sécurix](https://github.com/arcanesys/securix) (ANSSI-hardened NixOS distribution for government laptops) composes cleanly under [`nixfleet.lib.mkHost`](https://github.com/arcanesys/nixfleet).

## Composition

Three layers, each owning its concerns:

| Layer | Source | Role |
|---|---|---|
| Generic role | `nixfleet-scopes.scopes.roles.endpoint` | base CLI tools + secrets wiring + impermanence option surface + operators scope |
| Distro modules | `securix.nixosModules.securix-base` (bundles lanzaboote, agenix, disko) + hardware SKU | ANSSI hardening, multi-operator users, VPN / PAM / audit, hardware profile |
| Host-specific | inline module in `flake.nix` | operators declaration, `securix.self` metadata, bootloader/filesystem overrides |

NixFleet itself stays oblivious to ANSSI, Sécurix's SKU registry, lanzaboote, etc. It just composes NixOS modules.

## Operators

Users are declared via `nixfleet.operators`:

```nix
nixfleet.operators = {
  primaryUser = "operator";
  users.operator = {
    isAdmin = false;
    homeManager.enable = false;
  };
};
```

For multi-operator endpoints, add more users. Admin operators (`isAdmin = true`) get the `wheel` group. Use `rootSshKeys` for infrastructure SSH access to root.

## Securix dependencies

`securix.nixosModules.securix-base` bundles lanzaboote, agenix, and disko — no need to import them separately. Securix defaults (lanzaboote, mutableUsers, operators/vpnProfiles args) use `mkDefault`, so consumers override cleanly.

## Build

```sh
# Pure evaluation (fast, no VM build):
nix flake check --no-build

# Full system closure (slow, builds everything):
nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel

# Deploy to fresh hardware:
nixos-anywhere --flake .#lab-endpoint root@<ip>
```

## VM testing

Test the endpoint in a graphical QEMU VM with SPICE display.

**Before first use:** replace the placeholder SSH key with your own public key:

```sh
sed -i 's|ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|g' flake.nix
```

Then build and start the VM:

```sh
nix run .#build-vm -- -h lab-endpoint --ssh-port 2250 --disk-size 30G   # install via ISO + nixos-anywhere
nix run .#start-vm -- -h lab-endpoint --display gtk --ram 4096           # boot with GTK window
```

KDE Plasma needs at least 30G disk and 4G RAM. Use `--ssh-port` to avoid conflicts with other running VMs.

A GTK window opens with the VM display. The VM runs in the foreground — closing the window stops the VM. SSH is also available on the assigned port. Login: `operator` / `changeme`.

Other VM commands:

```sh
nix run .#stop-vm -- -h lab-endpoint           # stop the VM
nix run .#clean-vm -- -h lab-endpoint          # delete disk and state
nix run .#build-vm -- -h lab-endpoint --rebuild --ssh-port 2250 --disk-size 30G  # wipe and reinstall
```

See the [apps reference](../../docs/mdbook/reference/apps.md) for all flags.

## Extending

To adapt this example for a real endpoint:

1. Replace `securix.self.user` / `machine` with real values (email, serial, inventory ID, hardware SKU).
2. Add operator SSH keys and `rootSshKeys` for remote management.
3. Replace the stub `fileSystems."/"` with a proper disko config.
4. Remove `boot.lanzaboote.enable = false` after generating Secure Boot keys (`sbctl create-keys`).
5. Consider adding `isVm = true` for QEMU testing before real hardware.

### Supported Sécurix hardware SKUs

- `x280`, `elitebook645g11`, `elitebook850g8`, `latitude5340`, `t14g6`, `x9-15`, `e14-g7`

## See also

- [NixFleet](https://github.com/arcanesys/nixfleet) — `mkHost` and framework services
- [`nixfleet-scopes`](https://github.com/arcanesys/nixfleet-scopes) — generic roles and infrastructure scopes
- [Sécurix](https://github.com/arcanesys/securix) — ANSSI-hardened NixOS distribution
