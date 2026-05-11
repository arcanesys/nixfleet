# Wraps mkHost with qemu test-rig fixtures so the framework's public
# mkHost API stays free of VM-specific config.
{
  inputs,
  lib,
}: let
  mkHost = import ../../lib/mk-host.nix {inherit inputs lib;};

  qemuTestRigModules = [
    ../fixtures/qemu/disk-config.nix
    ../fixtures/qemu/hardware-configuration.nix
    ({
      lib,
      pkgs,
      ...
    }: {
      services.spice-vdagentd.enable = true;
      networking.useDHCP = lib.mkForce true;
      environment.variables.LIBGL_ALWAYS_SOFTWARE = "1";
      environment.systemPackages = [pkgs.mesa];
    })
  ];
in
  args @ {modules ? [], ...}:
    mkHost (args
      // {
        modules = qemuTestRigModules ++ modules;
        isVm = true;
      })
