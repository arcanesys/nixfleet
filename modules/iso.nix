# Custom NixOS minimal ISO with SSH key pre-configured for automated installs.
# Available as `packages.iso` on Linux systems only.
# Fleet sets `nixfleet.isoSshKeys` to bake its SSH keys into the ISO.
{
  inputs,
  config,
  lib,
  ...
}: {
  options.nixfleet.isoSshKeys = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [];
    description = "SSH public keys baked into the installer ISO for passwordless root access.";
  };

  config.perSystem = {
    system,
    lib,
    ...
  }: let
    isLinux = builtins.elem system ["x86_64-linux" "aarch64-linux"];
    keys = config.nixfleet.isoSshKeys;
  in
    lib.optionalAttrs (isLinux && keys != []) {
      packages.iso = let
        isoSystem = inputs.nixpkgs.lib.nixosSystem {
          modules = [
            "${inputs.nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
            {
              nixpkgs.hostPlatform = system;

              # SSH keys for passwordless root access (ISO only)
              users.users.root.openssh.authorizedKeys.keys = keys;
              services.openssh = {
                enable = true;
                settings.PermitRootLogin = "prohibit-password";
              };

              # QEMU guest support
              services.qemuGuest.enable = true;
              services.spice-vdagentd.enable = true;

              # Useful tools for installation
              environment.systemPackages = let
                pkgs = import inputs.nixpkgs {inherit system;};
              in [
                pkgs.git
                pkgs.parted
                pkgs.vim
              ];
            }
          ];
        };
      in
        isoSystem.config.system.build.isoImage;
    };
}
