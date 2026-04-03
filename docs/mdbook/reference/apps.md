# Apps

Flake apps provided by NixFleet. Available via `nix run .#<app>`. VM apps (`spawn-qemu`, `test-vm`, `spawn-utm`) are also exported via `nixfleet.lib.mkVmApps` for fleet repos.

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

## spawn-qemu

Launch a QEMU virtual machine. Linux only.

```sh
nix run .#spawn-qemu
nix run .#spawn-qemu -- --persistent -h web-02
nix run .#spawn-qemu -- --iso path/to/nixos.iso
nix run .#spawn-qemu -- --console
```

### Modes

**Basic mode** (default): Boot from an existing disk image or ISO.

**Persistent mode** (`--persistent -h HOST`): Build, install via nixos-anywhere, and launch a named host with a persistent disk stored at `~/.local/share/nixfleet/vms/<HOST>.qcow2`. On subsequent runs, boots the existing disk unless `--rebuild` is specified.

Persistent mode steps:
1. Build custom ISO
2. Create disk image
3. Boot from ISO (headless)
4. Install via nixos-anywhere
5. Launch graphical VM (SPICE)

If a key is found at `~/.keys/id_ed25519` or `~/.ssh/id_ed25519`, it is provisioned into the VM for secrets decryption.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--iso <PATH>` | string | -- | Boot from ISO (manual install) |
| `--disk <PATH>` | string | `qemu-disk.qcow2` | Disk image path |
| `--ram <MB>` | string | `4096` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |
| `--ssh-port <N>` | string | `2222` | Host port for SSH forwarding |
| `--disk-size <S>` | string | `20G` | Disk size for new images |
| `--console` | bool | -- | Headless mode (serial console, no GUI) |
| `--graphical` | bool | -- | GPU-accelerated GUI via SPICE (default) |
| `--persistent` | bool | -- | Persistent mode |
| `-h <HOST>` | string | -- | Host config to install (requires `--persistent`) |
| `--rebuild` | bool | -- | Wipe and reinstall (persistent mode only) |
| `--help` | -- | -- | Show help |

Graphical mode uses SPICE with `remote-viewer` and EGL headless rendering.

---

## test-vm

Automated VM test cycle: build ISO, boot, install, reboot, verify, cleanup. Linux only.

```sh
nix run .#test-vm
nix run .#test-vm -- -h web-02
nix run .#test-vm -- -h edge-01 --keep
```

### Steps

1. Build custom ISO
2. Create ephemeral disk (20G)
3. Boot QEMU from ISO (headless, SSH on port 2222)
4. Install via nixos-anywhere
5. Reboot from disk
6. Verify: hostname, `multi-user.target`, `sshd`

Cleans up temp directory and disk on exit unless `--keep` is specified.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | `edge-01` | Host config to install |
| `--keep` | bool | `false` | Keep temp dir and disk after test |
| `--ssh-port <N>` | string | `2222` | Host port for SSH |
| `--ram <MB>` | string | `4096` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |
| `--help` | -- | -- | Show help |

---

## spawn-utm

Launch or manage a UTM virtual machine. macOS only.

```sh
nix run .#spawn-utm -- --host myhost --start
nix run .#spawn-utm -- --ip
```

Requires UTM installed at `/Applications/UTM.app`.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | `nixos` | UTM VM name |
| `--host <HOST>` | string | -- | Host config |
| `--start` | bool | -- | Start the VM and detect IP |
| `--ip` | bool | -- | Print the VM's IP address |
| `--help` / `-h` | -- | -- | Show help |

### Actions

| Action | Description |
|--------|-------------|
| `setup` (default) | Print setup instructions |
| `start` (`--start`) | Start VM via `utmctl`, wait for IP (up to 60s) |
| `ip` (`--ip`) | Print current VM IP address |
