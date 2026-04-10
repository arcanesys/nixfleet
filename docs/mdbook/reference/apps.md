# Apps

Flake apps provided by NixFleet. Available via `nix run .#<app>`. VM lifecycle apps (`build-vm`, `start-vm`, `stop-vm`, `clean-vm`, `test-vm`) are exported via `nixfleet.lib.mkVmApps` for fleet repos.

## validate

Runs the full validation suite: formatting, eval tests, host builds, and optionally VM tests.

```sh
nix run .#validate
nix run .#validate -- --vm
nix run .#validate -- --fast
```

| Flag | Description |
|------|-------------|
| `--fast` | Reserved for future use |
| `--vm` | Include VM integration tests (`vm-core`, `vm-minimal`) |

### Checks performed

1. **Formatting** -- `nix fmt -- --fail-on-change`
2. **Eval tests** (Linux only) -- `eval-hostspec-defaults`, `eval-ssh-hardening`, `eval-username-override`, `eval-locale-timezone`, `eval-ssh-authorized`, `eval-password-files`
3. **NixOS test hosts** -- Builds `system.build.toplevel` for every host in `nixosConfigurations`
4. **VM tests** (Linux only, with `--vm`) -- `vm-core`, `vm-minimal`

Reports pass/fail/skip counts. Exits with code 1 if any check fails.

---

## build-vm

Install a host into a persistent QEMU disk via nixos-anywhere. Linux and macOS.

```sh
nix run .#build-vm -- -h web-02
nix run .#build-vm -- -h web-02 --rebuild
nix run .#build-vm -- --all
```

Steps:
1. Build custom ISO
2. Create disk image at `~/.local/share/nixfleet/vms/<HOST>.qcow2`
3. Boot QEMU from ISO (headless, SSH forwarded)
4. Install via nixos-anywhere
5. Stop ISO VM

If a disk already exists, the install is skipped unless `--rebuild` is specified. If a key is found at `~/.keys/id_ed25519` or `~/.ssh/id_ed25519`, it is provisioned into the VM for secrets decryption.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host config to install |
| `--all` | bool | -- | Install all hosts in nixosConfigurations |
| `--rebuild` | bool | -- | Wipe and reinstall existing disk |
| `--identity-key <PATH>` | string | -- | Path to identity key for secrets decryption |
| `--ssh-port <N>` | string | auto | Override SSH port (default: auto-assigned by index) |
| `--ram <MB>` | string | `4096` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |
| `--disk-size <S>` | string | `20G` | Disk size |

---

## start-vm

Start an installed VM as a headless daemon. Linux and macOS.

```sh
nix run .#start-vm -- -h web-02
nix run .#start-vm -- --all
```

Boots from the existing disk created by `build-vm`. SSH is forwarded to a per-host port (auto-assigned by sorted nixosConfigurations index, base 2201).

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host to start |
| `--all` | bool | -- | Start all installed VMs |
| `--ssh-port <N>` | string | auto | Override SSH port |
| `--ram <MB>` | string | `1024` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |

---

## stop-vm

Stop a running VM daemon. Linux and macOS.

```sh
nix run .#stop-vm -- -h web-02
nix run .#stop-vm -- --all
```

Sends SIGTERM to the QEMU process and removes the pidfile.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host to stop |
| `--all` | bool | -- | Stop all running VMs |

---

## clean-vm

Delete VM disk, pidfile, and port file. Linux and macOS.

```sh
nix run .#clean-vm -- -h web-02
nix run .#clean-vm -- --all
```

Stops the VM if running, then removes `<HOST>.qcow2`, `<HOST>.pid`, and `<HOST>.port` from `~/.local/share/nixfleet/vms/`.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host to clean |
| `--all` | bool | -- | Clean all VMs |

---

## test-vm

Automated VM test cycle: build ISO, boot, install, reboot, verify, cleanup. Linux and macOS.

```sh
nix run .#test-vm -- -h web-02
nix run .#test-vm -- -h edge-01 --keep
```

### Steps

1. Build custom ISO
2. Create ephemeral disk (20G)
3. Boot QEMU from ISO (headless, SSH on port 2299)
4. Install via nixos-anywhere
5. Reboot from disk
6. Verify: hostname, `multi-user.target`, `sshd`

Cleans up temp directory and disk on exit unless `--keep` is specified.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host config to install |
| `--keep` | bool | `false` | Keep temp dir and disk after test |
| `--ssh-port <N>` | string | `2299` | Host port for SSH |
| `--identity-key <PATH>` | string | -- | Path to identity key for secrets decryption |
| `--ram <MB>` | string | `4096` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |



> **Note:** Provisioning real hardware is done via standard NixOS tooling: `nixos-anywhere --flake .#hostname root@ip`. See [Standard Tools](../guide/deploying/standard-tools.md).
