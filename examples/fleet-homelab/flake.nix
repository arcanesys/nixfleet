{
  description = "nixfleet homelab example — exercises lib/mkFleet.nix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixfleet.url = "path:../..";
    nixfleet.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    nixfleet,
    ...
  }: {
    nixosConfigurations = {
      m70q-attic = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [./hosts/m70q.nix];
      };
      workstation = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [./hosts/workstation.nix];
      };
      rpi-sensor-01 = nixpkgs.lib.nixosSystem {
        system = "aarch64-linux";
        modules = [./hosts/rpi-sensor.nix];
      };
    };

    fleet = import ./fleet.nix {
      inherit self nixfleet;
    };
  };
}
