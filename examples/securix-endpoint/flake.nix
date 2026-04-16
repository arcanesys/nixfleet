# Example: a Sécurix-hardened endpoint under NixFleet `mkHost`.
#
# This example proves the 3-way composition:
#   (1) Generic role   — `nixfleet-scopes.scopes.roles.endpoint`
#                        provides the bare posture: base CLI tools +
#                        secrets wiring + impermanence option surface.
#                        It deliberately does NOT set up a user model,
#                        firewall, or filesystems — that's the distro's
#                        job.
#
#   (2) Distro modules — `securix.nixosModules.securix-base` brings
#                        ANSSI hardening, multi-operator user model,
#                        VPN / PAM / audit / etc.
#                        `securix.nixosModules.securix-hardware.t14g6`
#                        is Sécurix's own internal SKU module — just
#                        another NixOS module to NixFleet.
#
#   (3) Host-specific  — the inline module at the end carries the bits
#       tweaks          that are specific to this one lab machine:
#                        host identity, `securix.self` metadata, and a
#                        handful of workarounds for upstream Sécurix
#                        requirements that don't fit the pilot shape
#                        (see README for the full list).
#
# The takeaway: NixFleet does not need to know anything about ANSSI,
# Sécurix's SKU registry, or lanzaboote. It just composes three NixOS
# modules into `nixosSystem` and lets each layer own its concerns.
#
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
{
  description = "Sécurix endpoint under NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:abstracts33d/nixfleet";
    nixfleet-scopes.url = "github:arcanesys/nixfleet-scopes";
    # TODO: revert to `github:arcanesys/securix` once the flake-wrapper
    # PR (arcanesys/securix#1) lands on main. Today the flake.nix only
    # exists on the feat/flake-wrapper branch.
    securix.url = "github:arcanesys/securix/feat/flake-wrapper";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {
    nixfleet,
    nixfleet-scopes,
    securix,
    ...
  }: {
    nixosConfigurations.lab-endpoint = nixfleet.lib.mkHost {
      hostName = "lab-endpoint";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "operator";
        timeZone = "Europe/Paris";
        locale = "fr_FR.UTF-8";
        keyboardLayout = "fr";
        sshAuthorizedKeys = [];
      };
      modules = [
        # (1) Generic role from nixfleet-scopes
        nixfleet-scopes.scopes.roles.endpoint

        # (2) Distro modules from Sécurix
        securix.nixosModules.securix-base
        securix.nixosModules.securix-hardware.t14g6

        # Sécurix pulls these in through its own flake inputs; we
        # surface them here so the composition is explicit.
        securix.inputs.lanzaboote.nixosModules.lanzaboote
        "${securix.inputs.agenix}/modules/age.nix"
        "${securix.inputs.disko}/module.nix"

        # (3) Host-specific tweaks + Sécurix workarounds
        ({lib, ...}: {
          # Sécurix "self" metadata — identifies this physical machine
          # to Sécurix's ANSSI-compliance tracking and hardware routing.
          securix.self = {
            mainDisk = "/dev/vda";
            edition = "pilot";
            user = {
              email = "operator@example.gouv.fr";
              username = "operator";
            };
            machine = {
              serialNumber = "PILOT0001";
              inventoryId = 1;
              hardwareSKU = "t14g6";
              users = [];
            };
          };

          # Sécurix DE choice — default to KDE for the pilot.
          securix.graphical-interface.variant = lib.mkDefault "kde";

          # Workarounds — see README.md for rationale.
          _module.args = {
            operators = {};
            vpnProfiles = {};
          };
          fileSystems."/" = lib.mkForce {
            device = "/dev/vda1";
            fsType = "ext4";
          };
          boot.lanzaboote.enable = lib.mkOverride 0 false;
          boot.loader.systemd-boot.enable = lib.mkOverride 0 true;
          users.allowNoPasswordLogin = true;

          system.stateVersion = "24.11";
        })
      ];
    };
  };
}
