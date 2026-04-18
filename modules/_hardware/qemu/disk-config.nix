# Qemu VM disk layout for nixfleet's test hosts (isVm = true).
# Uses the btrfs-impermanence disk template from nixfleet-scopes.
# Imports the disko NixOS module (mk-host no longer auto-injects it).
{
  inputs,
  lib,
  ...
}: let
  diskConfig =
    import inputs.nixfleet-scopes.scopes.disk-templates.btrfs-impermanence
    {
      inherit lib;
    };
in {
  imports = [inputs.disko.nixosModules.disko];
  disko.devices = diskConfig;
}
