# Installation

Detailed installation guide for all platforms.

## macOS (nix-darwin)

Build and activate the darwin configuration:

```sh
darwin-rebuild switch --flake .#<hostname>
```

**What happens:**
1. SSH key is verified (needed for secrets repo)
2. nix-darwin configuration is built and activated
3. Home Manager configures your user environment

**After install:** Open a new terminal. Your shell, prompt, and tools are ready.

## NixOS (Remote via nixos-anywhere)

Boot the target machine from any NixOS ISO (or the custom ISO with `nix build .#iso`), then:

```sh
nixos-anywhere --flake .#<hostname> root@<ip>
```

**What happens:**
1. SSH connectivity is verified
2. Your agenix decryption key is provisioned via `--extra-files`
3. nixos-anywhere partitions disks (via disko) and installs NixOS
4. On reboot, secrets are decrypted, WiFi connects, everything works

**Options:**
- `--build-on-remote` -- build on the target machine (useful for cross-platform installs)
- Custom SSH port: use `--ssh-port <port>`

## NixOS (Rebuild)

For an already-installed NixOS host:

```sh
sudo nixos-rebuild switch --flake .#<hostname>
```

## Custom ISO

Build an ISO with your SSH key baked in for passwordless install:

```sh
nix build .#iso
```

This ISO boots with your SSH key in `authorized_keys`, so nixos-anywhere can connect without a password.

## VM Setup

See [VM Testing](../development/vm-testing.md) for QEMU and UTM setup.

## Troubleshooting

- **SSH fails:** Ensure your key is in the agent (`ssh-add -l`)
- **Secrets missing:** Check `~/.keys/id_ed25519` exists (the agenix decryption key)
- **Build fails:** Run `git add .` -- Nix only sees git-tracked files

For technical details on each host configuration, see the [Technical Docs](../../hosts/README.md).
