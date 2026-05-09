{
  config,
  pkgs,
  lib,
  ...
}: let
  hS = config.hostSpec;
in {
  imports = [../../contracts/trust.nix];

  # GOTCHA: mkDefault on package so distro forks (Lix, Determinate) swap without mkForce.
  nix = {
    nixPath = lib.mkDefault [];
    package = lib.mkDefault pkgs.nix;
    extraOptions = ''
      experimental-features = nix-command flakes
    '';
  };

  networking.hostName = hS.hostName;
  networking.interfaces = lib.mkIf (hS.networking ? interface) {
    "${hS.networking.interface}".useDHCP = lib.mkDefault true;
  };

  time.timeZone = hS.timeZone;
  i18n.defaultLocale = hS.locale;
  console.keyMap = lib.mkDefault hS.keyboardLayout;
  services.xserver.xkb.layout = lib.mkDefault hS.keyboardLayout;

  users.users.root = {
    openssh.authorizedKeys.keys = hS.rootSshKeys;
    hashedPasswordFile =
      lib.mkIf (hS.rootHashedPasswordFile != null)
      hS.rootHashedPasswordFile;
  };
}
