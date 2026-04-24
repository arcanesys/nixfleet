{
  description = "nixfleet spike — mkFleet + reconciler";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs = inputs @ {
    self,
    nixpkgs,
    flake-parts,
    ...
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux" "aarch64-linux"];

      flake = {
        lib.mkFleet = (import ./lib/mkFleet.nix {lib = nixpkgs.lib;}).mkFleet;

        # Stub nixosConfigurations — real ones would live in your main flake.
        nixosConfigurations = {
          m70q-attic = nixpkgs.lib.nixosSystem {
            system = "x86_64-linux";
            modules = [./examples/homelab/hosts/m70q-attic.nix];
          };
          workstation = nixpkgs.lib.nixosSystem {
            system = "x86_64-linux";
            modules = [./examples/homelab/hosts/workstation.nix];
          };
          rpi-sensor-01 = nixpkgs.lib.nixosSystem {
            system = "aarch64-linux";
            modules = [./examples/homelab/hosts/rpi-sensor-01.nix];
          };
        };

        fleet = import ./examples/homelab/fleet.nix {
          nixfleet = self;
          inherit self;
        };
      };

      perSystem = {pkgs, ...}: {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [cargo rustc rust-analyzer jq nixfmt-classic];
        };
      };
    };
}
