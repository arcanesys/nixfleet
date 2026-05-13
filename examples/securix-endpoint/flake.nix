# Example: a Sécurix-hardened endpoint composed via NixFleet `mkHost`.
#
# mkHost is platform-agnostic - Sécurix's NixOS modules drop in like any
# other module. The framework stays oblivious to ANSSI, hardware SKUs,
# lanzaboote, etc.; the example proves the composition holds.
#
# Build:   nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
# Deploy:  nixos-anywhere --flake .#lab-endpoint root@<ip>
# VM test: nix run .#build-vm -- -h lab-endpoint
#          nix run .#start-vm -- -h lab-endpoint --display gtk --ram 4096
#
# Before booting: replace the placeholder SSH key with your own:
#   sed -i 's|ssh-ed25519 NixfleetExampleKeyReplaceWithYourOwn|'"$(cat ~/.ssh/id_ed25519.pub)"'|g' host.nix
{
  description = "Sécurix endpoint composed via NixFleet mkHost";

  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    # Sécurix's flake wrapper lives on feat/flake-cleanup until merged
    # to main; exposes nixosModules.{securix-base, securix-hardware.<sku>}.
    securix.url = "github:arcanesys/securix/feat/flake-cleanup";
    securix.inputs.nixpkgs.follows = "nixfleet/nixpkgs";
    nixpkgs.follows = "nixfleet/nixpkgs";
    flake-parts.follows = "nixfleet/flake-parts";
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];

      flake.nixosConfigurations.lab-endpoint = inputs.nixfleet.lib.mkHost {
        hostName = "lab-endpoint";
        platform = "x86_64-linux";
        hostSpec = {
          timeZone = "Europe/Paris";
          locale = "fr_FR.UTF-8";
          # Matches Sécurix's default; otherwise both modules set
          # `console.keyMap` at the same priority and merge fails.
          keyboardLayout = "fr";
        };
        modules = [
          # Sécurix base - bundles lanzaboote + agenix + disko + the full
          # ANSSI module tree (anssi, bastion, vpn, pam, auditd, ...).
          inputs.securix.nixosModules.securix-base

          # SKU hardware profile. Pick from: e14-g7, elitebook645g11,
          # elitebook850g8, latitude5340, t14g6, x9-15, x280.
          # Omit on VM - vm-overrides.nix neutralizes the hardware bits.
          inputs.securix.nixosModules.securix-hardware.t14g6

          # Host-specific: operators + securix.self metadata + agent.
          ./host.nix

          # VM-only overrides (disable Secure Boot + LUKS, set up disko).
          # Drop this import for a real-hardware deploy.
          ./vm-overrides.nix
        ];
      };

      perSystem = {
        pkgs,
        system,
        ...
      }: {
        apps = inputs.nixfleet.lib.mkVmApps {inherit pkgs;};

        # Minimal installer ISO with a placeholder root SSH key - needed
        # by `build-vm` (which uses ISO + nixos-anywhere). Replace the key
        # with your own; or, for a real-hardware deploy, skip this and
        # drive nixos-anywhere directly with any installer.
        packages.iso = let
          isoSystem = inputs.nixpkgs.lib.nixosSystem {
            modules = [
              "${inputs.nixpkgs}/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix"
              {
                nixpkgs.hostPlatform = system;
                users.users.root.openssh.authorizedKeys.keys = [
                  "ssh-ed25519 NixfleetExampleKeyReplaceWithYourOwn"
                ];
                services.openssh.enable = true;
                services.openssh.settings.PermitRootLogin = "prohibit-password";
                services.qemuGuest.enable = true;
                services.spice-vdagentd.enable = true;
              }
            ];
          };
        in
          isoSystem.config.system.build.isoImage;
      };
    };
}
