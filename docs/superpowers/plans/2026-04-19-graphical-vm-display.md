# Graphical VM Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--display spice|gtk|none` flag to `start-vm` so graphical VMs (e.g., securix endpoints with KDE) can be interactively tested.

**Architecture:** A `--display` flag on `start-vm` switches QEMU display backend. SPICE mode adds virtio-vga + SPICE agent for clipboard, runs foreground (no daemonize). The securix-endpoint example gets `mkVmApps` wired in.

**Tech Stack:** Nix (bash script generation via `mk-vm-apps.nix`), QEMU, SPICE

---

### Task 1: Add `--display` flag and display args to `start-vm`

**Files:**
- Modify: `modules/_shared/lib/mk-vm-apps.nix:317-384`

- [ ] **Step 1: Add `spice-gtk` to basePkgs conditionally**

In `mk-vm-apps.nix`, add `spice-gtk` to a new `spicePkgs` binding after `basePkgs` (line 82):

```nix
  spicePkgs = with pkgs; [spice-gtk];
```

- [ ] **Step 2: Add display arg helper to `sharedHelpers`**

At the end of `sharedHelpers` (before the closing `''`), add a function that computes QEMU display args:

```bash
    compute_display_args() {
      DISPLAY_ARGS=""
      DAEMONIZE_ARGS="-daemonize"
      case "''${DISPLAY_MODE:-none}" in
        spice)
          DISPLAY_ARGS="-display spice-app -device virtio-vga -chardev spicevmc,id=vdagent,debug=0,name=vdagent -device virtio-serial-pci -device vdagent,chardev=vdagent,name=com.redhat.spice.0"
          DAEMONIZE_ARGS=""
          ;;
        gtk)
          DISPLAY_ARGS="-display gtk -device virtio-vga"
          DAEMONIZE_ARGS=""
          ;;
        none)
          DISPLAY_ARGS="-display none -serial null"
          ;;
        *)
          echo -e "''${RED}Unknown display mode: ''$DISPLAY_MODE (use: none, spice, gtk)''${NC}" >&2
          exit 1
          ;;
      esac
    }
```

- [ ] **Step 3: Add `--display` to `start-vm` argument parser**

In the `start-vm` script, add `DISPLAY_MODE="none"` to the variable declarations (after `VLAN_PORT=""`), and add the case to the arg parser:

```bash
      DISPLAY_MODE="none"
```

In the `while` loop, add before the `*` catch-all:

```bash
          --display) DISPLAY_MODE="$2"; shift 2 ;;
```

- [ ] **Step 4: Add `spice-gtk` to PATH when SPICE is selected**

After the `${sharedHelpers}` line in `start-vm`, add:

```bash
      [[ "''${DISPLAY_MODE:-none}" == "spice" ]] && export PATH="${lib.makeBinPath spicePkgs}:$PATH"
```

Wait — this won't work because `DISPLAY_MODE` is parsed later. Move this after the arg parsing loop instead. Add after the `done` of the while loop and before the `[[ $ALL -eq 0 ...` check:

```bash
      [[ "$DISPLAY_MODE" == "spice" ]] && export PATH="${lib.makeBinPath spicePkgs}:$PATH"
```

- [ ] **Step 5: Replace hardcoded display/daemonize in `start_one`**

In the `start_one` function, add a call to `compute_display_args` after `compute_vlan_args`:

```bash
        compute_display_args
```

Replace the QEMU invocation (lines 362-371) with:

```bash
        ${qemuBin} \
          ${qemuAccel} \
          -m "''$RAM" \
          -smp "''$CPUS" \
          -drive file="$disk",format=qcow2,if=virtio \
          -nic user,model=virtio-net-pci,hostfwd=tcp::''$SSH_PORT-:22 \
          ''$VLAN_ARGS \
          ''$DISPLAY_ARGS \
          -bios ${qemuFirmware} \
          ''$DAEMONIZE_ARGS -pidfile "$pidfile"
```

- [ ] **Step 6: Update the success message for foreground mode**

After the QEMU invocation, adjust the message to account for foreground mode:

```bash
        if [ -n "$DAEMONIZE_ARGS" ]; then
          echo -e "''${GREEN}[$host] Started on port ''$SSH_PORT — ssh -p ''$SSH_PORT root@localhost''${NC}"
        else
          echo -e "''${GREEN}[$host] Running in foreground (port ''$SSH_PORT) — close the viewer to stop''${NC}"
        fi
```

- [ ] **Step 7: Update usage hint for `--all` + display incompatibility**

After the `[[ "$DISPLAY_MODE" == "spice" ]]` PATH line, add a guard:

```bash
      if [[ $ALL -eq 1 && "$DISPLAY_MODE" != "none" ]]; then
        echo -e "''${RED}--display requires -h HOST (not --all)''${NC}" >&2
        exit 1
      fi
```

- [ ] **Step 8: Verify syntax**

Run: `nix eval .#apps.x86_64-linux.start-vm.program --raw`

Expected: a store path to the `start-vm` script (no eval errors).

- [ ] **Step 9: Commit**

```bash
git add modules/_shared/lib/mk-vm-apps.nix
git commit -m "feat(vm): add --display spice|gtk|none flag to start-vm"
```

---

### Task 2: Wire `mkVmApps` into securix-endpoint example

**Files:**
- Modify: `examples/securix-endpoint/flake.nix`

- [ ] **Step 1: Add `apps` output to the example flake**

In `examples/securix-endpoint/flake.nix`, modify the `outputs` block. After the `nixosConfigurations` attribute (line 87, before the closing `};`), add:

```nix
    apps.x86_64-linux = nixfleet.lib.mkVmApps {pkgs = nixpkgs.legacyPackages.x86_64-linux;};
```

This requires `nixpkgs` to be available in the outputs function args. It already is via `...`, but we need the binding. Update the outputs function signature (line 22-27) to include `nixpkgs`:

```nix
  outputs = {
    nixfleet,
    nixfleet-scopes,
    securix,
    nixpkgs,
    ...
  }: {
```

- [ ] **Step 2: Update the header comment**

Replace lines 8-10 of the header comment:

```nix
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
```

with:

```nix
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display spice --ram 4096
```

- [ ] **Step 3: Commit**

```bash
git add examples/securix-endpoint/flake.nix
git commit -m "feat(examples): wire mkVmApps into securix-endpoint"
```

---

### Task 3: Update documentation

**Files:**
- Modify: `docs/mdbook/reference/apps.md:72-92`

- [ ] **Step 1: Update `start-vm` section**

Replace the `start-vm` section (lines 72-92) with:

```markdown
## start-vm

Start an installed VM. Runs headless by default; use `--display` for graphical output. Linux and macOS.

```sh
nix run .#start-vm -- -h web-02
nix run .#start-vm -- -h web-02 --display spice --ram 4096
nix run .#start-vm -- --all
```

Boots from the existing disk created by `build-vm`. SSH is forwarded to a per-host port (auto-assigned by sorted nixosConfigurations index, base 2201).

When `--display` is `spice` or `gtk`, the VM runs in the foreground (no daemonize). Closing the viewer window stops the VM. SPICE mode provides clipboard sharing via the SPICE agent.

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-h <HOST>` | string | -- | Host to start |
| `--all` | bool | -- | Start all installed VMs (headless only) |
| `--ssh-port <N>` | string | auto | Override SSH port |
| `--ram <MB>` | string | `1024` | RAM in MB |
| `--cpus <N>` | string | `2` | CPU count |
| `--display <MODE>` | string | `none` | Display: `none` (headless), `spice` (SPICE viewer), `gtk` (native window) |
```

- [ ] **Step 2: Commit**

```bash
git add docs/mdbook/reference/apps.md
git commit -m "docs(apps): document --display flag on start-vm"
```

---

### Task 4: Manual verification

- [ ] **Step 1: Verify `start-vm` eval**

Run: `nix eval .#apps.x86_64-linux.start-vm.program --raw`

Expected: a nix store path (no eval errors).

- [ ] **Step 2: Test headless mode unchanged**

Start any existing test VM headless:

```bash
nix run .#start-vm -- -h web-01
```

Verify it daemonizes and SSH works. Stop it:

```bash
nix run .#stop-vm -- -h web-01
```

- [ ] **Step 3: Test SPICE mode**

Start with SPICE (requires a built VM disk):

```bash
nix run .#start-vm -- -h web-01 --display spice --ram 2048
```

Verify: SPICE viewer window opens, VM boots, SSH still works on the forwarded port. Close the viewer — VM process should exit.

- [ ] **Step 4: Test `--all` + `--display` guard**

```bash
nix run .#start-vm -- --all --display spice
```

Expected: error message "--display requires -h HOST (not --all)" and exit code 1.
