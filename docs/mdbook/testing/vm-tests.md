# VM Tests (Tier A)

## Purpose

Boot NixOS VMs and assert runtime state: services running, binaries present, configs deployed. Uses `pkgs.testers.nixosTest`.

## Location

- `modules/tests/vm.nix`

## Test Suites

### vm-core
Non-minimal node. Tests:
- `multi-user.target` active
- `sshd` running
- `NetworkManager` running
- iptables Chain INPUT present (firewall enabled)
- `testuser` exists and is in wheel group
- zsh and git available to the test user

### vm-minimal
Node with `isMinimal = true`. Tests:
- `multi-user.target` active
- `sshd` running
- Core services (NetworkManager, firewall) active

## mkTestNode Helper

Builds nixosTest-compatible node configs with:
- All framework NixOS + HM modules included
- Agenix secrets stubbed (`lib.mkForce {}`)
- Known test password ("test") for user and root
- nixpkgs with `allowUnfree = true`

## Platform

x86_64-linux only (nixosTest requirement).

## Running

```sh
nix run .#validate -- --vm
```

Or directly:

```sh
nix build .#checks.x86_64-linux.vm-core --no-link
nix build .#checks.x86_64-linux.vm-minimal --no-link
```

## Links

- [Testing Overview](README.md)
- [Eval Tests](eval-tests.md)
