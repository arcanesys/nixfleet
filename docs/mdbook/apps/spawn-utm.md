# spawn-utm

## Purpose

UTM virtual machine setup guide and helper for macOS. UTM uses Apple Virtualization Framework for aarch64-linux VMs. Since UTM's automation API is limited, this app provides guided instructions and IP detection.

## Location

- `modules/apps.nix` (the `spawn-utm` app definition)
- **Platform:** Darwin only

## Usage

```sh
nix run .#spawn-utm                          # setup guide
nix run .#spawn-utm -- --ip                  # get VM IP
nix run .#spawn-utm -- --start               # start VM and show IP
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--name NAME` | nixos | VM name in UTM |
| `--host NAME` | web-02 | NixOS host config |
| `--start` | -- | Start VM, wait for IP |
| `--ip` | -- | Show running VM IP |

## Setup Flow

1. Download aarch64 ISO
2. Create VM in UTM (Virtualize > Linux, 4GB RAM, 64GB disk, shared network)
3. Boot, set root password
4. Get VM IP with `--ip`
5. Install: `nixos-anywhere --flake .#<hostname> root@<ip>`

## Dependencies

- UTM.app (`/Applications/UTM.app/Contents/MacOS/utmctl`)

## Links

- [Apps Overview](README.md)
