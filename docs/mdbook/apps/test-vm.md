# test-vm

## Purpose

Automated end-to-end VM test cycle: build custom ISO, boot QEMU, install via nixos-anywhere, reboot from disk, verify installation. All in one command with cleanup.

## Location

- `modules/apps.nix` (the `test-vm` app definition)
- **Platform:** Linux only

## Usage

```sh
nix run .#test-vm                          # test with 'edge-01' host (default)
nix run .#test-vm -- -h web-02             # test with web-02
nix run .#test-vm -- -h edge-01 --keep     # keep disk for inspection
```

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `-h HOST` | edge-01 | Host config to install |
| `--keep` | -- | Keep temp dir and disk after test |
| `--ssh-port N` | 2222 | SSH port |
| `--ram MB` | 4096 | RAM |
| `--cpus N` | 2 | CPU count |

## Test Steps

1. **Build ISO** -- `nix build .#iso` (custom ISO with SSH key)
2. **Create disk** -- ephemeral qcow2 in temp dir
3. **Boot QEMU** -- daemonized, headless (`-display none`), ISO boot
4. **nixos-anywhere** -- install to VM disk, no reboot
5. **Reboot** -- kill QEMU, restart from disk
6. **Verify** -- hostname matches, multi-user.target active, sshd active

## Dependencies

- [ISO builder](../architecture.md) (`modules/iso.nix`)
- nixos-anywhere
- qemu, openssh

## Links

- [Apps Overview](README.md)
- [VM hosts](../hosts/vm/README.md)
