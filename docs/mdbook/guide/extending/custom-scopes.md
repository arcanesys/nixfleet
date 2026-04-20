# Custom Scopes

Scopes are plain NixOS/HM modules that self-activate based on enable options. The framework provides `base`, `impermanence`, and the service modules. Your fleet repo adds scopes for everything else.

## Step 1: Define a hostSpec flag

Extend hostSpec in your fleet repo with a plain NixOS module:

```nix
# modules/host-spec-extensions.nix
{lib, ...}: {
  options.hostSpec.isDev = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = "Enable development tools.";
  };
}
```

Include this module in your mkHost `modules` list (or use an import-tree pattern).

## Step 2: Create the scope module

Write a NixOS module that activates only when the flag is true:

```nix
# modules/scopes/dev.nix
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isDev {
    virtualisation.docker.enable = true;
    environment.systemPackages = with pkgs; [gcc gnumake];
  };
}
```

## Step 3: Add Home Manager config

If the scope needs user-level configuration, use the HM module pattern. You can define it as a separate module or combine it with the NixOS module depending on your import strategy.

In a multi-module pattern (returned as an attrset):

```nix
# modules/scopes/dev.nix
{
  nixos = {config, lib, pkgs, ...}: let
    hS = config.hostSpec;
  in {
    config = lib.mkIf hS.isDev {
      virtualisation.docker.enable = true;
    };
  };

  homeManager = {config, lib, pkgs, ...}: let
    hS = config.hostSpec;
  in {
    home.packages = lib.optionals hS.isDev (with pkgs; [
      nodejs
      python3
      rustup
    ]);
  };
}
```

## Step 4: Add persist paths

If the scope installs programs with state on impermanent hosts, co-locate the persistence declaration:

```nix
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isDev {
    virtualisation.docker.enable = true;

    # Persist Docker data on impermanent hosts
    environment.persistence."/persist".directories =
      lib.mkIf config.nixfleet.impermanence.enable [
        "/var/lib/docker"
      ];
  };
}
```

For user-level persistence (in an HM module):

```nix
home.persistence."/persist" = lib.mkIf config.nixfleet.impermanence.enable {
  directories = [".cargo" ".rustup" ".npm"];
};
```

## Step 5: Import in mkHost

Add the scope module to your host definitions:

```nix
nixfleet.lib.mkHost {
  hostName = "workstation";
  platform = "x86_64-linux";
  hostSpec = {
    userName = "alice";
    isDev = true;
  };
  modules = [
    ./modules/host-spec-extensions.nix
    ./modules/scopes/dev.nix
    ./hardware-configuration.nix
  ];
}
```

If you use an import-tree or similar auto-discovery pattern, the scope is picked up automatically without explicit imports.

## Conventions

- **One concern per scope** - `dev`, `graphical`, `desktop`, not `dev-and-graphical`
- **`lib.mkIf` on enable options** - scopes produce no config when their enable is false
- **Co-locate persistence** - persist paths live in the scope that needs them
- **Framework vs fleet** - generic infrastructure (base, impermanence, agent, CP) belongs in NixFleet. Opinionated tools and theming belong in your fleet repo.
