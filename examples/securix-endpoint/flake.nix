# Example: a Sécurix-hardened endpoint under NixFleet `mkHost`.
#
# Three-layer composition:
#   (1) Generic role  - `nixfleet.scopes.roles.endpoint`
#   (2) Distro        - `securix.nixosModules.securix-base` (bundled deps)
#                        + hardware SKU module
#   (3) Host-specific - operators, securix.self metadata, overrides
#
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display gtk --ram 4096
#
# Before booting: replace the placeholder SSH key with your own public key:
#   sed -i 's|ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|g' flake.nix host.nix
{
  description = "Sécurix endpoint under NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    securix.url = "github:arcanesys/securix";
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
          # (1) Generic role
          inputs.nixfleet.scopes.roles.endpoint

          # (2) Distro modules from Sécurix (deps bundled in securix-base)
          inputs.securix.nixosModules.securix-base
          inputs.securix.nixosModules.securix-hardware.t14g6

          # (3) Host-specific
          ./host.nix

          # (4) VM overrides (omit for real hardware)
          ./vm-overrides.nix
        ];
      };

      perSystem = {pkgs, ...}: {
        apps = inputs.nixfleet.lib.mkVmApps {inherit pkgs;};
      };
    };
}
