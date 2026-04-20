# The mkHost API

`mkHost` is the single entry point for defining hosts in NixFleet. It is a closure over framework inputs (nixpkgs, home-manager, disko, impermanence, microvm) that returns a standard `nixosSystem` or `darwinSystem`.

The result is a standard NixOS/Darwin system configuration. All existing NixOS tooling (`nixos-rebuild`, `nixos-anywhere`, `darwin-rebuild`) works unchanged.

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `hostName` | string | yes | Machine hostname |
| `platform` | string | yes | `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin` |
| `stateVersion` | string | no | NixOS/Darwin state version (default: `"24.11"`) |
| `hostSpec` | attrset | no | Host configuration flags. See [hostSpec](hostspec.md) |
| `modules` | list | no | Additional NixOS/Darwin modules |
| `isVm` | bool | no | Inject QEMU VM hardware (default: `false`) |

For the full parameter reference, injected module order, return types, Home Manager integration, and exports, see the [mkHost API reference](../../reference/mkhost-api.md).

## Examples

### Single host

The simplest pattern. One machine, one repo, no fleet infrastructure.

```nix
# flake.nix
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

Deploy with standard NixOS tooling:

```sh
nixos-anywhere --flake .#myhost root@192.168.1.50   # fresh install
sudo nixos-rebuild switch --flake .#myhost            # local rebuild
```

### Multi-host fleet with org defaults

Define shared defaults in a `let` binding and merge per-host overrides. This example uses flake-parts.

```nix
# fleet.nix (flake-parts module)
{config, ...}: let
  mkHost = config.flake.lib.mkHost;

  acme = {
    userName = "deploy";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
  };
in {
  flake.nixosConfigurations = {
    dev-01 = mkHost {
      hostName = "dev-01";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [
        nixfleet-scopes.scopes.roles.workstation
        { nixfleet.impermanence.enable = true; }
        ./hosts/dev-01/hardware.nix
        ./hosts/dev-01/disk-config.nix
      ];
    };

    prod-web-01 = mkHost {
      hostName = "prod-web-01";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [
        nixfleet-scopes.scopes.roles.server
        ./hosts/prod-web-01/hardware.nix
        ./hosts/prod-web-01/disk-config.nix
      ];
    };
  };
}
```

### Batch hosts from a template

Standard Nix. Generate 50 identical edge devices with `builtins.genList`, then merge with named hosts.

```nix
# fleet.nix (flake-parts module)
{config, ...}: let
  mkHost = config.flake.lib.mkHost;

  acme = {
    userName = "deploy";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
  };

  edgeHosts = builtins.listToAttrs (map (i: {
    name = "edge-${toString i}";
    value = mkHost {
      hostName = "edge-${toString i}";
      platform = "aarch64-linux";
      hostSpec = acme;
      modules = [
        nixfleet-scopes.scopes.roles.endpoint
        ./hosts/edge/common-hardware.nix
        ./hosts/edge/disk-config.nix
      ];
    };
  }) (builtins.genList (i: i + 1) 50));

  namedHosts = {
    control-plane = mkHost {
      hostName = "control-plane";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [ nixfleet-scopes.scopes.roles.server ./hosts/control-plane/hardware.nix ];
    };
  };
in {
  flake.nixosConfigurations = namedHosts // edgeHosts;
}
```

No special batch API needed - `mkHost` is a plain function, and Nix handles the rest.

## Key points

- **hostSpec values use `lib.mkDefault`**, so modules you pass in `modules` can override them.
- **`hostName` is the exception** - it is set without `mkDefault` and always matches the `hostName` parameter.
- **`isDarwin` is auto-detected** from the `platform` parameter. You never set it manually.
- **VM mode** (`isVm = true`) adds QEMU hardware, SPICE agent, DHCP, and software GL - useful for testing with `nix run .#build-vm` and `nix run .#start-vm`.
