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
{
  description = "Sécurix endpoint under NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixfleet-scopes.url = "github:arcanesys/nixfleet-scopes";
    # TODO: revert to github:arcanesys/securix once feat/flake-cleanup merges
    securix.url = "github:arcanesys/securix/feat/flake-cleanup";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {
    nixfleet,
    nixfleet-scopes,
    securix,
    ...
  }: {
    nixosConfigurations.lab-endpoint = nixfleet.lib.mkHost {
      hostName = "lab-endpoint";
      platform = "x86_64-linux";
      hostSpec = {
        timeZone = "Europe/Paris";
        locale = "fr_FR.UTF-8";
        keyboardLayout = "fr";
      };
      modules = [
        # (1) Generic role from nixfleet-scopes
        nixfleet-scopes.scopes.roles.endpoint

        # (2) Distro modules from Sécurix (deps bundled in securix-base)
        securix.nixosModules.securix-base
        securix.nixosModules.securix-hardware.t14g6

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
  };
}
