# LOADBEARING: mk-fleet.nix takes only {lib}; pure consumers import it directly to avoid the inputs dependency.
{
  inputs,
  lib,
}: let
  mkFleetImpl = import ./mk-fleet.nix {inherit lib;};
in {
  mkHost = import ./mk-host.nix {inherit inputs lib;};
  mkVmApps = import ./mk-vm-apps.nix {inherit inputs;};
  inherit (mkFleetImpl) mkFleet mergeFleets withSignature;
}
