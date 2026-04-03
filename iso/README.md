# NixOS Installer ISO

Custom NixOS minimal ISO with SSH key pre-configured for automated installs.

## Build

```sh
nix build .#iso
```

The ISO includes:
- Our SSH public key in root's authorized_keys (no passwd needed)
- QEMU guest agents + SPICE support
- Git, parted, vim

## Usage

```sh
# Install a host using the ISO automatically (build ISO + install via nixos-anywhere)
nix run .#build-vm -- -h <hostname>

# Fully automated (build ISO + install + reboot + verify + cleanup)
nix run .#test-vm -- -h web-02
```
