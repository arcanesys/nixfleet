# Graphical VM Display Support

Add `--display` flag to `start-vm` for interactive graphical VM sessions (SPICE/GTK). Primary use case: testing securix endpoints with KDE Plasma in a QEMU VM.

## Scope

### In scope

- `--display spice|gtk|none` flag on `start-vm` (default: `none`)
- SPICE: auto-launch viewer, virtio GPU, clipboard agent
- GTK: native window, virtio GPU
- Foreground mode when display is not `none` (no `-daemonize`)
- Wire `examples/securix-endpoint/` to use `mkVmApps`
- Documentation update for `--display` flag

### Out of scope

- No changes to `build-vm`, `test-vm`, `stop-vm`, `clean-vm`
- No new apps
- No changes to `fleet.nix` test hosts
- No securix repo changes

## Framework Change: `mk-vm-apps.nix`

### Flag parsing in `start-vm`

Add `--display` to the argument parser (default `none`). Accepted values: `spice`, `gtk`, `none`.

### QEMU arguments by display mode

**`none` (default, unchanged):**
```
-display none -serial null
```

**`spice`:**
```
-display spice-app
-device virtio-vga
-chardev spicevmc,id=vdagent,debug=0,name=vdagent
-device virtio-serial-pci
-device vdagent,chardev=vdagent,name=com.redhat.spice.0
```

**`gtk`:**
```
-display gtk
-device virtio-vga
```

### Foreground behavior

When `--display` is not `none`, omit `-daemonize`. The VM runs in the foreground -- closing the SPICE viewer or GTK window stops the VM. The pidfile is still written so `stop-vm` works if the user backgrounds the process manually.

### Dependencies

Add `spice-gtk` to the script PATH when SPICE display is selected. This provides the `spice-client-glib` backend that QEMU's `-display spice-app` needs. GTK mode uses QEMU's built-in GTK display (no extra deps).

### Help text update

Add `--display` to the usage output:
```
--display MODE   Display mode: none (default), spice, gtk
```

## Example Flake Change: `examples/securix-endpoint/flake.nix`

Add one line to the outputs:
```nix
apps.x86_64-linux = nixfleet.lib.mkVmApps { pkgs = nixpkgs.legacyPackages.x86_64-linux; };
```

Update the header comment to include VM testing commands:
```
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display spice --ram 4096
```

## Documentation

Update `docs/mdbook/reference/apps.md`:
- Document `--display spice|gtk|none` flag on `start-vm`
- Note that SPICE mode runs in foreground (no daemonize)
- Mention host requirements: `spice-gtk` package (provided automatically via nix)
- Recommend `--ram 4096` or higher for graphical DEs

## Testing

Manual verification:
1. Build the securix endpoint: `nix run .#build-vm -- -h lab-endpoint`
2. Start with SPICE: `nix run .#start-vm -- -h lab-endpoint --display spice --ram 4096`
3. Verify SPICE viewer launches and KDE desktop is usable
4. Verify `--display none` (default) behavior is unchanged
5. Verify `stop-vm` still works when VM was started in foreground mode
