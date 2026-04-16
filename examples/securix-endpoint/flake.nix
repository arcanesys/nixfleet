# Example: a Sécurix-hardened endpoint deployed via nixfleet.lib.mkHost.
#
# This is the acceptance artifact for the Sécurix × NixFleet pilot (phase 0/1).
# It demonstrates consuming Sécurix base modules + a hardware profile
# under mkHost, with the endpoint escape-hatches enabled.
#
# Eval (fast):
#   nix eval .#nixosConfigurations.lab-endpoint.config.system.build.toplevel.drvPath
#
# Full build (slow — user runs manually):
#   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
#
# For real hardware deployment: see README.md.
{
  description = "Sécurix endpoint under NixFleet mkHost — pilot acceptance example";

  inputs = {
    nixfleet.url = "github:abstracts33d/nixfleet/feat/endpoint-escape-hatches";
    securix.url = "github:arcanesys/securix/feat/flake-wrapper";
    nixpkgs.follows = "nixfleet/nixpkgs";
    securix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    nixfleet,
    securix,
  }: {
    nixosConfigurations.lab-endpoint = nixfleet.lib.mkHost {
      hostName = "lab-endpoint";
      platform = "x86_64-linux";
      isVm = true; # eval-friendly; flip to false for real hardware

      hostSpec = {
        userName = "operator";
        timeZone = "Europe/Paris";
        locale = "fr_FR.UTF-8";
        keyboardLayout = "fr";
        sshAuthorizedKeys = [];

        # Endpoint escape-hatches (phase 1 additions)
        managedUser = false;
        enableHomeManager = false;
        customFilesystems = true;
        skipDefaultFirewall = true;
      };

      modules = [
        # Sécurix base + one hardware profile
        securix.nixosModules.securix-base
        securix.nixosModules.securix-hardware.t14g6

        # Companion modules required by securix-base (see README for why)
        securix.inputs.lanzaboote.nixosModules.lanzaboote
        "${securix.inputs.agenix}/modules/age.nix"
        "${securix.inputs.disko}/module.nix"

        # Endpoint-specific wiring
        ({lib, ...}: {
          # Required Sécurix options
          securix.self = {
            mainDisk = "/dev/vda"; # VM; use /dev/nvme0n1 on real hw
            edition = "pilot";
            user = {
              email = "operator.pilot@example.gouv.fr";
              # Explicit username (derivation from email has bugs with domain dots)
              username = "operator";
            };
            machine = {
              serialNumber = "PILOT0001";
              inventoryId = 1;
              hardwareSKU = "t14g6";
              users = []; # inventory disabled for single-operator pilot
            };
          };

          # graphical-interface.variant needs an explicit value even when disabled
          securix.graphical-interface.variant = lib.mkDefault "kde";

          # _module.args injected by mkTerminal in the native Sécurix path;
          # here we inject them inline for mkHost.
          _module.args = {
            operators = {};
            vpnProfiles = {};
          };

          # Minimal VM filesystem (endpoint hostSpec.customFilesystems = true
          # keeps NixFleet's qemu disk-config from being imported, so we
          # provide one here).
          fileSystems."/" = {
            device = "/dev/vda1";
            fsType = "ext4";
          };

          # Sécurix enables lanzaboote with mkForce; disable for VM eval (no TPM).
          # mkOverride 0 wins over Sécurix's mkForce (priority 50).
          boot.lanzaboote.enable = lib.mkOverride 0 false;
          boot.loader.systemd-boot.enable = lib.mkOverride 0 true;

          # Sécurix's default user has hashedPassword = "!" (locked).
          # In a VM smoke, allow no-password login so the "locked out" assertion passes.
          users.allowNoPasswordLogin = true;

          system.stateVersion = "24.11";
        })
      ];
    };
  };
}
