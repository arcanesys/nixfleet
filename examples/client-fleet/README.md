# Example: Client Fleet

This is a minimal example showing how an organization would consume NixFleet as a framework.

## Structure

```
client-fleet/
├── fleet.nix        # Imports nixfleet, defines hosts via mkHost (flake-parts module)
└── README.md
```

## How it works

1. **`fleet.nix`** is a flake-parts module that calls `mkHost` per host
2. **Org defaults** are defined as a `let` binding and merged into each host's `hostSpec`
3. **Roles** (server, workstation, endpoint) configure scope defaults for each host type
4. **Operators** define the user inventory - team members, SSH keys, admin privileges
5. Fleet repos add their own modules for secrets, scopes, and customization

The framework provides all core modules (SSH hardening, firewall, identity) and scopes (base, impermanence, agent, control-plane). The client only defines what is specific to their organization.

## Consumption pattern

```nix
# fleet.nix (flake-parts module)
{config, inputs, ...}: let
  mkHost = config.flake.lib.mkHost;

  orgDefaults = {
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
  };

  operatorsModule = {
    nixfleet.operators = {
      primaryUser = "deploy";
      rootSshKeys = [ "ssh-ed25519 AAAA..." ];
      users.deploy = {
        isAdmin = true;
        homeManager.enable = false;
        sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
      };
    };
  };
in {
  flake.nixosConfigurations = {
    web-01 = mkHost {
      hostName = "web-01";
      platform = "x86_64-linux";
      hostSpec = orgDefaults;
      modules = [
        inputs.nixfleet.scopes.roles.server
        operatorsModule
        ./hosts/web-01/hardware.nix
      ];
    };

    dev-workstation = mkHost {
      hostName = "dev-workstation";
      platform = "x86_64-linux";
      hostSpec = orgDefaults;
      modules = [
        inputs.nixfleet.scopes.roles.workstation
        operatorsModule
        ./hosts/dev-workstation/hardware.nix
      ];
    };
  };
}
```

## Deployment

```sh
# Fresh install
nixos-anywhere --flake .#web-01 root@192.168.1.10

# Rebuild
sudo nixos-rebuild switch --flake .#web-01

# macOS
darwin-rebuild switch --flake .#<hostname>
```
