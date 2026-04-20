# Public API of the NixFleet framework library.
{
  inputs,
  lib,
}: {
  mkHost = import ./mk-host.nix {inherit inputs lib;};
  mkVmApps = import ./mk-vm-apps.nix {inherit inputs;};
}
