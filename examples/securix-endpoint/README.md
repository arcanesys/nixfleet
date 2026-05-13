# `securix-endpoint` - Sécurix endpoint under NixFleet

End-to-end example proving that [Sécurix](https://github.com/arcanesys/securix) (ANSSI-hardened NixOS for government laptops) composes cleanly under `nixfleet.lib.mkHost`. NixFleet stays oblivious to ANSSI, hardware SKUs, lanzaboote; it just composes NixOS modules.

## Composition

```nix
modules = [
  inputs.securix.nixosModules.securix-base           # ANSSI + bundled deps
  inputs.securix.nixosModules.securix-hardware.t14g6 # SKU profile
  ./host.nix                                          # operators + securix.self
  ./vm-overrides.nix                                  # VM-only (omit for real HW)
];
```

`securix-base` brings lanzaboote, agenix, disko, and the full Sécurix module tree (anssi, bastion, vpn, pam, auditd, ...). No separate inputs needed.

## Hardware SKUs

Pick from: `e14-g7`, `elitebook645g11`, `elitebook850g8`, `latitude5340`, `t14g6`, `x9-15`, `x280`. On VM, omit the hardware module - `vm-overrides.nix` neutralizes the hardware bits.

## Build

```sh
# Pure eval (fast, no VM build):
nix eval .#nixosConfigurations.lab-endpoint.config.system.build.toplevel.drvPath

# Build the system closure:
nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel

# VM lifecycle (Linux only):
# --disk-size 30G is required - the full Sécurix desktop closure
# (KDE + GStreamer + plasma + ...) doesn't fit in build-vm's 5G default.
nix run .#build-vm -- -h lab-endpoint --disk-size 30G
nix run .#start-vm -- -h lab-endpoint --display gtk --ram 4096
nix run .#stop-vm  -- -h lab-endpoint
```

## Deploy to real hardware

```sh
nixos-anywhere --flake .#lab-endpoint root@<ip>
```

Drop the `./vm-overrides.nix` import in `flake.nix` first - real hardware uses Sécurix's full LUKS + lanzaboote + impermanence layout.

## Customize before booting

Replace the placeholder SSH key (appears in `host.nix` for the installed
host, and in `flake.nix` for the bootstrap ISO):

```sh
sed -i 's|ssh-ed25519 NixfleetExampleKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|g' host.nix flake.nix
```

The VM's `operator` user has password `changeme`. Change `vm-overrides.nix`'s `hashedPassword` for anything other than throwaway VM testing.

## Enroll under a NixFleet control plane

The example doesn't enable `services.nixfleet-agent` by default - composition is the point. Uncomment the block in `host.nix` and point at your CP to enroll the host.
