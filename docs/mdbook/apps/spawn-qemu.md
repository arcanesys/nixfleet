# spawn-qemu

## Purpose

QEMU/KVM virtual machine launcher with GPU-accelerated graphics via SPICE (virgl) or headless serial console mode.

## Location

- `modules/apps.nix` (the `spawn-qemu` app definition)
- **Platform:** Linux only

## Usage

```sh
nix run .#spawn-qemu -- --iso iso/nixos-x86_64.iso   # boot from ISO
nix run .#spawn-qemu                                    # boot from disk
nix run .#spawn-qemu -- --console                       # headless mode
nix run .#spawn-qemu -- --persistent -h web-02          # persistent VM (replaces launch-vm)
nix run .#spawn-qemu -- --persistent -h dev-01 --rebuild  # wipe and reinstall
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--iso PATH` | -- | Boot from ISO |
| `--disk PATH` | qemu-disk.qcow2 | Disk image path |
| `--ram MB` | 4096 | RAM |
| `--cpus N` | 2 | CPU count |
| `--ssh-port N` | 2222 | SSH port forwarding |
| `--disk-size S` | 20G | New disk image size |
| `--console` | -- | Headless serial console |
| `--graphical` | (default) | GPU-accelerated SPICE |
| `--persistent` | -- | Persistent mode: build, install, and launch with a persistent disk |
| `-h HOST` | `web-02` | Host configuration to build (persistent mode) |
| `--rebuild` | -- | Wipe disk and reinstall from scratch (persistent mode) |

## Persistent Mode

Persistent mode (`--persistent`) replaces the former `launch-vm` app. It builds a host configuration, creates a persistent qcow2 disk, installs via nixos-anywhere, and boots with a SPICE graphical display.

1. If no disk exists (or `--rebuild`): builds the host, creates a qcow2 disk, installs via nixos-anywhere
2. Boots the VM with SPICE display (opens `remote-viewer` automatically)
3. Disk is persisted at `~/.local/share/nixfleet/vms/<host>.qcow2`

Use `-h HOST` to select the host configuration (defaults to `web-02`).

## Graphical Mode

Uses EGL headless rendering with virtio-vga-gl and SPICE on port 5900. Auto-launches `remote-viewer`. On non-NixOS, requires sudo to create `/run/opengl-driver` symlink for GBM drivers.

## Dependencies

- Packages: qemu, openssh, virt-viewer, mesa, OVMF
- SPICE on localhost:5900 (no auth -- acceptable for local dev)

## Migration from launch-vm

The `launch-vm` app has been removed. Use `spawn-qemu --persistent` instead:

| Old command | New command |
|---|---|
| `nix run .#launch-vm` | `nix run .#spawn-qemu -- --persistent` |
| `nix run .#launch-vm -- -h dev-01` | `nix run .#spawn-qemu -- --persistent -h dev-01` |
| `nix run .#launch-vm -- --rebuild` | `nix run .#spawn-qemu -- --persistent --rebuild` |

## Links

- [Apps Overview](README.md)
- [VM hosts](../hosts/vm/README.md)
