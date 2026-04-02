# VM Testing

Test your config in virtual machines before deploying to hardware.

## Automated Testing

The fastest way to verify a full install cycle:

```sh
nix run .#test-vm -- -h web-02
```

This runs a complete cycle: build custom ISO, boot QEMU, install via nixos-anywhere, reboot, verify SSH/hostname/services. Fully automated, no interaction needed.

## QEMU (Linux Host)

For interactive testing with a graphical VM:

```sh
# First boot from ISO
nix run .#spawn-qemu -- --iso iso/nixos-x86_64.iso

# After install, boot from disk
nix run .#spawn-qemu

# Headless mode (serial console)
nix run .#spawn-qemu -- --console
```

The graphical mode uses SPICE with virgl for GPU-accelerated rendering. SSH is forwarded to `localhost:2222`.

## UTM (macOS Host)

For testing on Apple Silicon:

```sh
# Guided setup
nix run .#spawn-utm

# Get VM IP after boot
nix run .#spawn-utm -- --ip

# Install into the VM
nixos-anywhere --flake .#web-02 root@<ip>
```

## NixOS VM Tests

Separate from the install-test VMs, the NixOS test framework runs integration tests:

```sh
nix run .#validate -- --vm
```

These tests use `nixosTest` to boot minimal VMs and verify specific behaviors. See [Testing](testing.md) for details on the test suites.

## Further Reading

- [Testing](testing.md) — the full test pyramid
- [Host System](../../hosts/README.md) -- test fleet and host configuration
