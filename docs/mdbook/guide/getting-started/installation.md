# Installation

NixFleet uses standard NixOS/Darwin tooling for installation. No custom deploy scripts.

## NixOS - Remote Install

Install a fresh machine over SSH using [nixos-anywhere](https://github.com/nix-community/nixos-anywhere):

```bash
nixos-anywhere --flake .#web-01 root@192.168.1.10
```

The target machine needs SSH access and must be booted into a NixOS installer or any Linux with `kexec` support. nixos-anywhere handles disk partitioning (via disko), NixOS installation, and the first boot.

### Options

```bash
# Provision extra files (e.g. host keys, pre-generated secrets)
nixos-anywhere --flake .#web-01 --extra-files ./secrets root@192.168.1.10

# Build on the remote machine (useful for aarch64 targets without cross-compilation)
nixos-anywhere --flake .#web-01 --build-on-remote root@192.168.1.10
```

## NixOS - Rebuild

For machines already running NixOS:

```bash
# Local rebuild
sudo nixos-rebuild switch --flake .#web-01

# Remote rebuild
nixos-rebuild switch --flake .#web-01 --target-host root@192.168.1.10
```

## macOS

For Darwin hosts (Apple Silicon or Intel), use nix-darwin:

```bash
darwin-rebuild switch --flake .#macbook
```

The `mkHost` function detects `aarch64-darwin` or `x86_64-darwin` platforms and calls `darwinSystem` instead of `nixosSystem`, injecting the appropriate Darwin core module and Home Manager integration.

## Custom ISO

Build an installer ISO with your fleet's SSH keys and base configuration pre-baked:

```bash
nix build .#iso
```

The resulting ISO is written to `result/iso/`. Flash it to USB and boot target machines for a known-good starting point before running `nixos-anywhere`.

## VM Testing

Test host configurations in QEMU before deploying to real hardware.

**Prerequisites:** Your fleet must set `nixfleet.isoSshKeys` with a public key whose private half is on your machine (`~/.ssh/id_ed25519.pub`). The `sshAuthorizedKeys` in your hostSpec should use the same key. VM commands SSH into the ISO installer using this key - if it doesn't match, SSH will hang.

```bash
# Install a host into a persistent VM disk (build ISO + nixos-anywhere)
nix run .#build-vm -- -h web-01

# Start the installed VM as a headless daemon
nix run .#start-vm -- -h web-01

# Full VM test cycle (build, install, reboot, verify, cleanup)
nix run .#test-vm -- -h web-01
```

See [VM Tests](../../testing/vm-tests.md) for details on writing VM test assertions.

## Troubleshooting

### SSH connection refused

nixos-anywhere requires root SSH access on the target. Verify:

```bash
ssh root@192.168.1.10 echo ok
```

If the target is a fresh installer image, root login is usually enabled by default. For existing systems, ensure `services.openssh.enable = true` and `users.users.root.openssh.authorizedKeys.keys` includes your public key.

### Build fails with "path not found"

Flakes only see files tracked by git. If you just created or moved files:

```bash
git add -A
```

Then retry the build.

### Missing state on impermanent hosts

Hosts with `nixfleet.impermanence.enable = true` wipe root on every boot. If a service loses state after reboot, its data directory must be added to the persistence configuration. The agent and control plane modules handle this automatically - their state directories (`/var/lib/nixfleet`, `/var/lib/nixfleet-cp`) are persisted when impermanence is active.

For other services, add persist paths in your modules:

```nix
environment.persistence."/persist".directories = [
  "/var/lib/my-service"
];
```
