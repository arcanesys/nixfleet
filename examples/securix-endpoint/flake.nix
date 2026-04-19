# Example: a Sécurix-hardened endpoint under NixFleet `mkHost`.
#
# Three-layer composition:
#   (1) Generic role  — `nixfleet-scopes.scopes.roles.endpoint`
#   (2) Distro        — `securix.nixosModules.securix-base` (bundled deps)
#                        + hardware SKU module
#   (3) Host-specific — operators, securix.self metadata, overrides
#
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display spice --ram 4096
#
# Before booting: replace the placeholder SSH key with your own public key:
#   sed -i 's|ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|' flake.nix
{
  description = "Sécurix endpoint under NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixfleet-scopes.url = "github:arcanesys/nixfleet-scopes";
    # TODO: revert to github:arcanesys/securix once feat/flake-cleanup merges
    securix.url = "github:arcanesys/securix/feat/flake-cleanup";
    nixpkgs.follows = "nixfleet/nixpkgs";
    flake-parts.follows = "nixfleet/flake-parts";
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];

      imports = [
        inputs.nixfleet.flakeModules.iso
      ];

      # SSH key baked into the installer ISO (replace with your own)
      nixfleet.isoSshKeys = [
        "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
      ];

      flake.nixosConfigurations.lab-endpoint = inputs.nixfleet.lib.mkHost {
        hostName = "lab-endpoint";
        platform = "x86_64-linux";
        hostSpec = {
          timeZone = "Europe/Paris";
          locale = "fr_FR.UTF-8";
          keyboardLayout = "fr";
        };
        modules = [
          # (1) Generic role from nixfleet-scopes
          inputs.nixfleet-scopes.scopes.roles.endpoint

          # (2) Distro modules from Sécurix (deps bundled in securix-base)
          inputs.securix.nixosModules.securix-base
          inputs.securix.nixosModules.securix-hardware.t14g6

          # (3) Host-specific
          ({lib, ...}: {
            # Operators — declarative user inventory
            nixfleet.operators = {
              primaryUser = "operator";
              users.operator = {
                isAdmin = false;
                homeManager.enable = false;
              };
            };

            # Sécurix identity metadata
            securix.self = {
              mainDisk = "/dev/vda";
              edition = "pilot";
              user = {
                email = "operator@example.gouv.fr";
                username = "operator";
              };
              machine = {
                serialNumber = "PILOT0001";
                inventoryId = 1;
                hardwareSKU = "t14g6";
                users = [];
              };
            };

            securix.graphical-interface.variant = lib.mkDefault "kde";

            # Override lanzaboote for VM testing (securix defaults to Secure
            # Boot via mkDefault — clean override, no mkOverride needed).
            boot.lanzaboote.enable = false;
            boot.loader.systemd-boot.enable = true;

            # Minimal VM filesystem (replace with disko for real deploys)
            fileSystems."/" = {
              device = "/dev/vda1";
              fsType = "ext4";
            };

            system.stateVersion = "24.11";
          })
        ];
      };

      perSystem = {pkgs, ...}: {
        apps = inputs.nixfleet.lib.mkVmApps {inherit pkgs;};
      };
    };
}
