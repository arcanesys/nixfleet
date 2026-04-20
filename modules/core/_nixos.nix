# Core NixOS module - framework mechanism only.
#
# Opinions (users, bootloader, programs, security, hardware) are gone:
# - User creation lives in `arcanesys/nixfleet-scopes` roles
#   (workstation / server) - consumers that want a primary user import
#   the appropriate role.
# - Bootloader config (systemd-boot, initrd modules, kernelPackages) is
#   left to host-specific modules (hardware-configuration.nix, disk
#   templates in nixfleet-scopes) where it belongs.
# - `programs.{zsh,git,gnupg,dconf}`, `security.sudo`,
#   `hardware.ledger` etc. are opinions and move downstream to fleet
#   scopes / Home Manager.
#
# What stays here:
# - nix settings (substituters, trusted keys, gc, experimental features)
#   so every NixOS host gets the NixFleet cache wiring out of the box.
# - openssh hardening (PermitRootLogin prohibit-password, password auth
#   off) - universally applicable and required for remote deploys.
# - Identity pass-through from hostSpec to NixOS options
#   (hostName, timeZone, locale, keyMap, xkb).
{
  config,
  pkgs,
  lib,
  ...
}: let
  hS = config.hostSpec;
in {
  # --- nixpkgs ---
  nixpkgs.config = lib.mkDefault {
    allowUnfree = true;
    allowBroken = false;
    allowInsecure = false;
    allowUnsupportedSystem = true;
  };

  # --- nix settings ---
  nix = {
    nixPath = lib.mkDefault [];
    settings = {
      trusted-users = ["@admin"];
      substituters = [
        "https://nix-community.cachix.org"
        "https://cache.nixos.org"
      ];
      trusted-public-keys = [
        "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
        "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
      ];
      auto-optimise-store = true;
    };
    # mkDefault so downstream distros (Sécurix uses Lix, etc.) can swap
    # the Nix implementation without mkForce ceremony.
    package = lib.mkDefault pkgs.nix;
    extraOptions = ''
      experimental-features = nix-command flakes
    '';
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 7d";
    };
  };

  # --- identity passthrough from hostSpec ---
  networking = {
    hostName = hS.hostName;
    useDHCP = false;
    interfaces = lib.mkIf (hS.networking ? interface) {
      "${hS.networking.interface}".useDHCP = true;
    };
    firewall.enable = lib.mkDefault true;
  };

  time.timeZone = hS.timeZone;
  i18n.defaultLocale = hS.locale;
  console.keyMap = lib.mkDefault hS.keyboardLayout;
  services.xserver.xkb.layout = lib.mkDefault hS.keyboardLayout;

  # --- openssh (hardened; universally applicable for fleet deploys) ---
  services.openssh = {
    enable = lib.mkDefault true;
    settings = {
      PermitRootLogin = lib.mkDefault "prohibit-password";
      PasswordAuthentication = lib.mkDefault false;
      KbdInteractiveAuthentication = lib.mkDefault false;
    };
  };

  # --- authorized_keys for root (identity-level access for deploys) ---
  # Root gets keys from `nixfleet.operators.rootSshKeys` - an explicit list
  # set by the fleet (typically seeded from admin operator keys via
  # `nixfleet.operators._adminSshKeys` in nixfleet-scopes). When no
  # operators scope is active (e.g. bare edge hosts), root falls back to
  # no managed keys - the consuming fleet must wire them directly.
  users.users.root = {
    openssh.authorizedKeys.keys =
      lib.mkIf (config ? nixfleet.operators.rootSshKeys)
      config.nixfleet.operators.rootSshKeys;
    hashedPasswordFile =
      lib.mkIf (hS.rootHashedPasswordFile != null)
      hS.rootHashedPasswordFile;
  };

  # --- minimal package set for remote-deploy ergonomics ---
  environment.systemPackages = with pkgs; [
    git
    inetutils
  ];
}
