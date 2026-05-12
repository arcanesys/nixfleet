# VM lifecycle

NixFleet ships `mkVmApps`, a helper that exposes a per-host VM lifecycle on the consuming fleet's `nix run` interface. Use it to exercise fleet configurations locally before deploying to real hardware.

## Wire mkVmApps into the consuming fleet

```nix
outputs = { nixpkgs, nixfleet, ... }: {
  # ... mkHost calls ...

  apps = nixfleet.lib.mkVmApps { inherit pkgs; };
};
```

Once wired, the following `nix run` subcommands are available on the consuming fleet.

## Subcommands

| Subcommand | What it does |
|---|---|
| `nix run .#build-vm -- -h <name>` | Boot the NixOS installer under QEMU, run `nixos-anywhere` to install the host's declared config to a fresh `qcow2` under `~/.local/share/nixfleet/vms/`, power off. Subsequent `start-vm` invocations boot the installed disk directly. |
| `nix run .#build-vm -- --all` | Build every VM declared in the fleet. |
| `nix run .#build-vm -- -h <name> --rebuild` | Wipe and reinstall (useful after `clean-vm` or after rotating a baked trust pin). |
| `nix run .#start-vm -- -h <name> [--vlan N]` | Boot a previously-built VM. `--vlan N` puts every VM on a shared QEMU multicast L2 so they resolve each other by hostname. |
| `nix run .#stop-vm -- -h <name>` | Power off a running VM. |
| `nix run .#stop-vm -- --all` | Power off every running VM. |
| `nix run .#clean-vm -- -h <name>` | Remove the VM's qcow2 + per-host state under `~/.local/share/nixfleet/vms/`. Must `build-vm` again before next `start-vm`. |
| `nix run .#test-vm -- -h <name>` | Run integration test scenarios against the VM (host-specific test set). |

## Per-host configuration

- **RAM** is declared per host in `hostSpec.vmRam` (default 1 GiB). Pass `--ram N` at runtime to override.
- **Port forwards** live in `hostSpec.vmPortForwards`. The host's SSH port is auto-assigned alphabetically (`2201 + index`).
- **VLAN** must match across every VM in a fleet that needs to resolve peers by hostname. Pass the same `--vlan N` to every `start-vm` invocation.

## Reference fleet

[nixfleet-demo](https://github.com/arcanesys/nixfleet-demo) exercises every subcommand end-to-end on a 4-VM reference fleet (`forge`, `cp`, `web-01`, `web-02`). The repo's README is a 10-step walkthrough from `build-vm` to a converged signed-GitOps loop - clone it as the fastest way to internalise the lifecycle.

## Common workflows

- **Iterate on a new module locally**: edit fleet config -> `nix run .#build-vm -- -h <name> --rebuild` -> `nix run .#start-vm -- -h <name>`.
- **Wipe and reinstall a single VM**: `nix run .#clean-vm -- -h <name>` then `nix run .#build-vm -- -h <name>`.
- **Spin up the full fleet locally**: `nix run .#build-vm -- --all` then `nix run .#start-vm -- -h <each>` (each with the same `--vlan`).
- **Wipe everything and restart**: `nix run .#stop-vm -- --all && nix run .#clean-vm -- --all && nix run .#build-vm -- --all`.

## Footguns

- **Darwin returns empty.** `mkVmApps` is a no-op on Darwin platforms; `aarch64-darwin` `pkgs.OVMF` is broken upstream. Build VMs on Linux hosts.
- **`clean-vm` wipes guest state.** Any keys generated on first boot inside the VM (release-signing keypair, agenix identity, host SSH keys) are gone. If you `clean-vm` a forge VM in the demo pattern, every downstream VM that baked the previous release-trust pin must be rebuilt too - they otherwise reject signatures with `BadSignature`.
- **VLAN mismatch is silent.** A typo in `--vlan` across VMs produces unresolvable hostnames with no obvious error message. All VMs in a fleet must use the same VLAN port number.
- **First CI run is slow.** When running the demo pattern (forge VM hosts a CI runner), the first push compiles the workspace from source - 20-45 minutes typical. Subsequent pushes are 2-5 minutes once the store is primed.
