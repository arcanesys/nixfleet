# The mkHost API

`mkHost` is the single entry point for defining hosts in NixFleet. It is a closure over framework inputs (nixpkgs, home-manager, disko, impermanence) that returns a standard `nixosSystem` or `darwinSystem`.

The result is a standard NixOS/Darwin system configuration. All existing NixOS tooling (`nixos-rebuild`, `nixos-anywhere`, `darwin-rebuild`) works unchanged.

## Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `hostName` | string | yes | -- | Machine hostname. Injected into `hostSpec.hostName` and `networking.hostName`. |
| `platform` | string | yes | -- | Target platform: `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin`. |
| `stateVersion` | string | no | `"24.11"` | NixOS/Home Manager state version. |
| `hostSpec` | attrset | no | `{}` | Host identity and capability flags. See [hostSpec Configuration](hostspec.md). |
| `modules` | list | no | `[]` | Additional NixOS/Darwin modules (hardware config, disk layout, fleet-specific modules). |
| `isVm` | bool | no | `false` | Inject QEMU VM hardware for testing. Adds SPICE, DHCP, and software GL. |

## What mkHost injects

Every call to `mkHost` automatically includes:

**NixOS hosts:**
- `hostSpec` module (host identity and capability options)
- Your `hostSpec` values (applied with `lib.mkDefault` so you can override in modules)
- disko and impermanence NixOS modules
- Core NixOS module (boot, networking, users, SSH, nix settings)
- Base scope (CLI packages, conditional on `!isMinimal`)
- Impermanence scope (btrfs root wipe, persist paths, conditional on `isImpermanent`)
- Agent and control-plane service modules (included but disabled by default)
- Home Manager with hostSpec, base HM module, and impermanence HM module

**Darwin hosts:**
- `hostSpec` module with `isDarwin = true` auto-set
- Core Darwin module (nix settings, system defaults, TouchID sudo, dock management)
- Base scope (Darwin-specific system packages)
- Home Manager with hostSpec and base HM module

Framework inputs are passed via `specialArgs`, so all injected modules can access `inputs`.

## Examples

### Single host

The simplest pattern. One machine, one repo, no fleet infrastructure.

```nix
# flake.nix
{
  inputs = {
    nixfleet.url = "github:your-org/nixfleet";
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
      hostSpec = acme // {
        isImpermanent = true;
      };
      modules = [
        ./hosts/dev-01/hardware.nix
        ./hosts/dev-01/disk-config.nix
      ];
    };

    prod-web-01 = mkHost {
      hostName = "prod-web-01";
      platform = "x86_64-linux";
      hostSpec = acme // {
        isServer = true;
        isMinimal = true;
      };
      modules = [
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
      hostSpec = acme // {
        isMinimal = true;
        isServer = true;
      };
      modules = [
        ./hosts/edge/common-hardware.nix
        ./hosts/edge/disk-config.nix
      ];
    };
  }) (builtins.genList (i: i + 1) 50));

  namedHosts = {
    control-plane = mkHost {
      hostName = "control-plane";
      platform = "x86_64-linux";
      hostSpec = acme // { isServer = true; };
      modules = [ ./hosts/control-plane/hardware.nix ];
    };
  };
in {
  flake.nixosConfigurations = namedHosts // edgeHosts;
}
```

No special batch API needed -- `mkHost` is a plain function, and Nix handles the rest.

## Key points

- **hostSpec values use `lib.mkDefault`**, so modules you pass in `modules` can override them.
- **`hostName` is the exception** -- it is set without `mkDefault` and always matches the `hostName` parameter.
- **`isDarwin` is auto-detected** from the `platform` parameter. You never set it manually.
- **VM mode** (`isVm = true`) adds QEMU hardware, SPICE agent, DHCP, and software GL -- useful for testing with `nix run .#build-vm` and `nix run .#start-vm`.
