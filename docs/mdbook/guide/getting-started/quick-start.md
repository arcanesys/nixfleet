# Quick Start

Get up and running with NixFleet.

## Prerequisites

- A machine with [Nix](https://nixos.org/download) installed (NixOS, macOS, or any Linux)
- A fleet repository consuming NixFleet (see [examples/](https://github.com/abstracts33d/nixfleet/tree/main/examples))

## Create Your Fleet

Start from the template:

```sh
nix flake init -t nixfleet
```

Or create a `flake.nix` that imports NixFleet and defines hosts with `mkHost`:

```nix
{
  inputs.nixfleet.url = "github:abstracts33d/nixfleet";
  inputs.nixpkgs.follows = "nixfleet/nixpkgs";

  outputs = { nixfleet, ... }: let
    mkHost = nixfleet.lib.mkHost;
  in {
    nixosConfigurations.my-host = mkHost {
      hostName = "my-host";
      platform = "x86_64-linux";
      hostSpec = { userName = "admin"; };
      modules = [ ./hardware/my-host ];
    };
  };
}
```

## Deploy

NixFleet uses standard NixOS deployment tooling — no custom wrappers. Any NixOS tutorial or documentation applies directly.

### NixOS (Remote)

Boot the target machine from a NixOS ISO, then:

```sh
nixos-anywhere --flake .#<hostname> root@<ip>
```

This handles disk partitioning (via disko), system configuration, and first boot.

### NixOS (Local rebuild)

```sh
sudo nixos-rebuild switch --flake .#<hostname>
```

### macOS

```sh
darwin-rebuild switch --flake .#<hostname>
```

### VM (Testing)

For a fully automated test cycle (build, install, verify):

```sh
nix run .#test-vm -- -h <hostname>
```

## Next Steps

- [Installation](installation.md) -- detailed install guide
- [Why NixOS?](../concepts/why-nixos.md) -- understand the philosophy
- [The Scope System](../concepts/scopes.md) -- how features are organized
