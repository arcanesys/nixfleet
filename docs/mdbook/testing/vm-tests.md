# VM Tests

VM tests boot real NixOS virtual machines and assert runtime state.
They verify that services start, users exist, packages are available, and
multi-node interactions work end-to-end.

## How to run

```sh
nix run .#validate -- --vm
```

Or build individual checks directly:

```sh
nix build .#checks.x86_64-linux.vm-core --no-link
nix build .#checks.x86_64-linux.vm-minimal --no-link
nix build .#checks.x86_64-linux.vm-nixfleet --no-link
nix build .#checks.x86_64-linux.vm-firewall --no-link
nix build .#checks.x86_64-linux.vm-monitoring --no-link
nix build .#checks.x86_64-linux.vm-backup --no-link
nix build .#checks.x86_64-linux.vm-secrets --no-link
```

## Requirements

- **Platform:** x86\_64-linux only (VM tests use QEMU under the hood)
- **KVM:** hardware virtualization recommended for performance (`/dev/kvm`)
- **Disk space:** each VM test builds a NixOS closure; expect several GB per test
- **Time:** minutes per test (build + boot + assertions + cleanup)

## Test cycle

Each VM test follows this sequence:

1. **Build** -- Nix evaluates the test node config and builds the NixOS closure
2. **Boot** -- QEMU launches the VM from the built closure
3. **Assert** -- a Python test script runs commands inside the VM via the
   NixOS test driver (`machine.succeed()`, `machine.fail()`, `machine.wait_for_unit()`)
4. **Cleanup** -- the VM shuts down and the test reports pass/fail

## Current tests

### vm-core

Boots a standard framework node (default `hostSpec`, no special flags) and verifies:

- `multi-user.target` reached
- `sshd` service running
- `NetworkManager` service running
- Firewall active (nftables ruleset has an input chain)
- Test user exists with `wheel` group membership
- `zsh` and `git` are available to the test user

### vm-minimal

Boots a node with `isMinimal = true` (negative test) and verifies:

- `multi-user.target` reached
- Core tools still present (`zsh`, `git` -- these come from `core/nixos.nix`, not
  the base scope)
- Graphical tools absent (e.g., `niri` not installed)
- Docker not running (no dev scope in the framework)

### vm-nixfleet

Two-node end-to-end test exercising the agent/control-plane cycle:

1. **Control plane** node starts, `nixfleet-control-plane.service` comes up on port 8080
2. **Agent** node starts with `pollInterval = 2`, `dryRun = true`, pointing at the CP
3. Test sets a fake desired generation on the CP via its HTTP API
4. Agent detects the generation mismatch, runs a dry-run cycle, and reports back
5. Test queries the CP inventory and asserts:
   - The agent appears in the machine list
   - `system_state == "ok"` (dry-run reports success)
   - `desired_generation` matches the fake hash that was set

### Infrastructure scope tests (vm-infra.nix)

Four focused tests for the infrastructure scopes:

**vm-firewall** — Verifies nftables is active, SSH rate limiting rules present (`limit rate 5/minute`), and drop logging enabled.

**vm-monitoring** — Enables the node exporter scope, verifies `prometheus-node-exporter.service` starts, port 9100 responds with Prometheus text format, and the `node_systemd` collector is active.

**vm-backup** — Enables the backup scope with a dummy `ExecStart` (`true`), verifies the systemd timer is registered, manually triggers the service, and checks that `status.json` is written with `"status": "success"`.

**vm-secrets** — Enables the secrets scope, verifies the SSH host key exists at `/etc/ssh/ssh_host_ed25519_key` with correct permissions (600).

## Test node construction

VM test nodes are built with `mkTestNode` from `modules/tests/_lib/helpers.nix`.
It mirrors what `mkHost` injects (core modules, scopes, disko, impermanence,
home-manager) but adds test-specific overrides:

- Known passwords (`"test"`) instead of hashed password files
- Explicit `nixpkgs` pinning for the test environment
- `hostSpecValues` passed directly (no `mkDefault` wrapping)

To add a new VM test, follow the pattern in `modules/tests/vm.nix`:

```nix
vm-my-test = pkgs.testers.nixosTest {
  name = "vm-my-test";
  nodes.machine = mkTestNode {
    hostSpecValues = defaultTestSpec // {
      # override flags as needed
    };
  };
  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.succeed("some-runtime-check")
  '';
};
```
