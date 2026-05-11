{
  inputs,
  lib,
  ...
}: let
  diskConfig =
    import ./disk-template.nix
    {
      inherit lib;
    };
in {
  imports = [inputs.disko.nixosModules.disko];
  disko.devices = diskConfig;
}
