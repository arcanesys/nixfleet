# Standard Tools

NixFleet builds on standard NixOS tooling. Every host produced by `mkHost` is a regular `nixosSystem` or `darwinSystem` output, so the standard deployment commands work unchanged.

## Fresh install (with disk partitioning)

```sh
nixos-anywhere --flake .#hostname root@192.168.1.42
```

Disko partitions the disk according to the host's disk config, then installs the NixOS closure.

## Local rebuild

```sh
sudo nixos-rebuild switch --flake .#hostname
```

## Remote rebuild

```sh
nixos-rebuild switch --flake .#hostname --target-host root@192.168.1.42
```

Evaluates locally, copies the closure to the target, and activates it.

## macOS rebuild

```sh
darwin-rebuild switch --flake .#hostname
```

## When to reach for more

These commands work because `mkHost` returns standard `nixosSystem`/`darwinSystem` outputs. The orchestration layer (control plane + agent) is additive - use it when your fleet grows beyond manual rebuilds.
