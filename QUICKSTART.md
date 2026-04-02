# Quick Start

Get NixFleet running in 5 minutes. For full documentation, see the [User Guide](docs/guide/README.md).

## Prerequisites

- [Nix](https://install.determinate.systems/nix) installed (with flakes enabled)
- Git

## Create a Fleet Repo

1. Create a `flake.nix` consuming nixfleet:

```nix
{
  inputs = {
    nixfleet.url = "github:your-org/nixfleet";
    nixpkgs.follows = "nixfleet/nixpkgs";
    home-manager.follows = "nixfleet/home-manager";
  };

  outputs = { nixfleet, ... }:
    let
      mkHost = nixfleet.lib.mkHost;
      orgDefaults = {
        userName = "admin";
        timeZone = "Europe/Paris";
        locale = "fr_FR.UTF-8";
      };
    in {
      nixosConfigurations.my-host = mkHost {
        hostName = "my-host";
        platform = "x86_64-linux";
        hostSpecValues = orgDefaults // {
          hostName = "my-host";
          isImpermanent = true;
        };
        hardwareModules = [ ./hardware/my-host ];
      };
    };
}
```

2. Define hosts with `mkHost`
3. Deploy with standard commands

## Deploy

```sh
# Fresh install (remote, via nixos-anywhere)
nixos-anywhere --flake .#hostname root@ip

# Rebuild NixOS
sudo nixos-rebuild switch --flake .#hostname

# Rebuild macOS
darwin-rebuild switch --flake .#hostname
```

## Explore

```sh
# List all host configurations
nix eval .#nixosConfigurations --apply 'x: builtins.attrNames x' --json | jq .

# Check available outputs
nix flake show
```

## Development

```sh
nix develop        # dev shell
nix fmt            # format all Nix files
nix flake check    # eval tests
nix run .#validate # full validation (format + eval + builds)
```

## Next Steps

- [README.md](README.md) -- Full feature overview
- [ARCHITECTURE.md](ARCHITECTURE.md) -- How the pieces fit together
- [TECHNICAL.md](TECHNICAL.md) -- Design decisions and Nix gotchas
- [CLAUDE.md](CLAUDE.md) -- Framework context, commands, conventions
- [docs/guide/](docs/guide/) -- Detailed user guide
- [docs/mdbook/](docs/mdbook/) -- Technical reference
