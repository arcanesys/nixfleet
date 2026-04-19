# Example: a Sécurix-hardened endpoint under NixFleet `mkHost`.
#
# Three-layer composition:
#   (1) Generic role  — `nixfleet-scopes.scopes.roles.endpoint`
#   (2) Distro        — `securix.nixosModules.securix-base` (bundled deps)
#                        + hardware SKU module
#   (3) Host-specific — operators, securix.self metadata, overrides
#
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display gtk --ram 4096
#
# Before booting: replace the placeholder SSH key with your own public key:
#   sed -i 's|ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|g' flake.nix
{
  description = "Sécurix endpoint under NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixfleet-scopes.follows = "nixfleet/nixfleet-scopes";
    # TODO: revert to github:arcanesys/securix once feat/flake-cleanup merges
    securix.url = "github:arcanesys/securix/feat/flake-cleanup";
    nixpkgs.follows = "nixfleet/nixpkgs";
    flake-parts.follows = "nixfleet/flake-parts";
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];

      imports = [
        inputs.nixfleet.flakeModules.iso
      ];

      # SSH key baked into the installer ISO (replace with your own)
      nixfleet.isoSshKeys = [
        "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
      ];

      flake.nixosConfigurations.lab-endpoint = inputs.nixfleet.lib.mkHost {
        hostName = "lab-endpoint";
        platform = "x86_64-linux";
        hostSpec = {
          timeZone = "Europe/Paris";
          locale = "fr_FR.UTF-8";
          keyboardLayout = "fr";
        };
        modules = [
          # (1) Generic role from nixfleet-scopes
          inputs.nixfleet-scopes.scopes.roles.endpoint

          # (2) Distro modules from Sécurix (deps bundled in securix-base)
          inputs.securix.nixosModules.securix-base
          inputs.securix.nixosModules.securix-hardware.t14g6

          # (3) Host-specific
          ({
            lib,
            pkgs,
            ...
          }: {
            # Operators — declarative user inventory
            nixfleet.operators = {
              primaryUser = "operator";
              rootSshKeys = [
                "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
              ];
              users.operator = {
                isAdmin = true;
                homeManager.enable = false;
                sshAuthorizedKeys = [
                  "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
                ];
              };
            };

            # Resolve shell conflict: operators scope and securix both set it
            # TODO: remove after arcanesys/nixfleet-scopes#7 merges
            users.users.operator.shell = lib.mkForce pkgs.zsh;

            # Sécurix identity metadata
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

            securix.graphical-interface.enable = true;
            securix.graphical-interface.variant = lib.mkDefault "kde";

            # Password for graphical login (SDDM) — "changeme"
            users.users.operator.hashedPassword = lib.mkForce "$6$gkBTmLDGP5NIkZpw$wgSG8D29EA1MfR6S27ypVq2ahAN9js3Fvsz.8auDlDlzR/P2mgsABIAicWMKf9JcT1p9VISXPkrfdvNg/VHDp1";

            # VM overrides — disable Secure Boot and LUKS (no TPM/passphrase in QEMU)
            boot.lanzaboote.enable = false;
            boot.loader.systemd-boot.enable = true;
            boot.initrd.availableKernelModules = ["virtio_pci" "virtio_blk" "virtio_scsi"];
            securix.filesystems.enable = false;
            disko.devices.disk.main = {
              device = "/dev/vda";
              type = "disk";
              content = {
                type = "gpt";
                partitions = {
                  ESP = {
                    end = "512M";
                    type = "EF00";
                    content = {
                      type = "filesystem";
                      format = "vfat";
                      mountpoint = "/boot";
                      mountOptions = ["umask=0077"];
                    };
                  };
                  root = {
                    size = "100%";
                    content = {
                      type = "filesystem";
                      format = "ext4";
                      mountpoint = "/";
                      extraArgs = ["-L" "nixos"];
                    };
                  };
                };
              };
            };

            system.stateVersion = "24.11";
          })
        ];
      };

      perSystem = {pkgs, ...}: {
        apps = inputs.nixfleet.lib.mkVmApps {inherit pkgs;};
      };
    };
}
