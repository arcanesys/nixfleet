# Templates & Patterns

NixFleet ships flake templates for common fleet structures. Initialize a new project with:

```sh
nix flake init -t github:arcanesys/nixfleet
```

## Available templates

| Template | Command | Description |
|----------|---------|-------------|
| `default` / `standalone` | `nix flake init -t nixfleet` | Single NixOS machine, no flake-parts |
| `fleet` | `nix flake init -t nixfleet#fleet` | Multi-host fleet with flake-parts |
| `batch` | `nix flake init -t nixfleet#batch` | Batch of identical hosts from a template |

### standalone

Minimal setup for a single machine. No flake-parts, no import-tree. Just nixfleet + one mkHost call:

```nix
{
  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {nixfleet, ...}: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "alice";
        timeZone = "US/Eastern";
        locale = "en_US.UTF-8";
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA..."
        ];
      };
      modules = [
        ./hardware-configuration.nix
        ./disk-config.nix
      ];
    };
  };
}
```

### fleet

Multi-host fleet using flake-parts for structure. Imports NixFleet's flakeModules for apps, tests, formatter, and ISO generation.

### batch

Generate many identical hosts from a template. Useful for edge devices, kiosks, or lab machines where the only difference between hosts is the hostname and network config.

## The follows chain

Every template uses this pattern:

```nix
inputs = {
  nixfleet.url = "github:arcanesys/nixfleet";
  nixpkgs.follows = "nixfleet/nixpkgs";
};
```

The `follows` directive means your fleet uses the same nixpkgs revision that NixFleet was tested against. This is important because:

- **Consistency** - framework modules, core config, and your fleet code all evaluate against the same package set
- **No diamond dependency** - without `follows`, you would have two separate nixpkgs evaluations (NixFleet's and yours), doubling memory usage and causing subtle version mismatches
- **Tested combination** - NixFleet's CI validates against its pinned nixpkgs

### NixFleet's own follows chain

NixFleet pins and follows these inputs internally:

```
nixpkgs           (nixos-unstable)
darwin            follows nixpkgs
home-manager      follows nixpkgs
disko             follows nixpkgs
impermanence      follows nixpkgs
lanzaboote        follows nixpkgs
microvm           follows nixpkgs
nixos-anywhere    follows nixpkgs, flake-parts, disko, treefmt-nix
treefmt-nix       follows nixpkgs
```

All major inputs share a single nixpkgs, ensuring consistent package versions throughout the dependency tree.

### When to follow vs pin independently

| Scenario | Recommendation |
|----------|---------------|
| Standard fleet | Follow NixFleet's nixpkgs (`follows = "nixfleet/nixpkgs"`) |
| Need a specific nixpkgs fix not yet in NixFleet | Pin your own nixpkgs, accept potential mismatches, update NixFleet soon |
| Fleet-specific inputs (secrets tool, hardware modules) | Follow your fleet's nixpkgs for consistency |
| NixFleet's bundled inputs (disko, HM, etc.) | Always use the versions bundled in NixFleet - they are tested together |

## Disko templates

NixFleet also provides reusable disk layout templates, separate from flake templates:

| Template | Import path | Description |
|----------|-------------|-------------|
| `btrfs` | `nixfleet.diskoTemplates.btrfs` | Standard btrfs layout |
| `btrfs-impermanence` | `nixfleet.diskoTemplates.btrfs-impermanence` | Btrfs with `@root`, `@persist`, `@nix` subvolumes for impermanence |

Use them in your mkHost `modules` list:

```nix
modules = [
  nixfleet.diskoTemplates.btrfs-impermanence
  ./hardware-configuration.nix
];
```
