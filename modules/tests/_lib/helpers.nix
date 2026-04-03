# Test helper functions for eval and VM checks.
# Usage: import from eval.nix, vm.nix, or vm-nixfleet.nix
{lib}: let
  # Import plain scope/core modules (same ones mkHost uses)
  baseScope = import ../../scopes/_base.nix;
  impermanenceScope = import ../../scopes/_impermanence.nix;
  firewallScope = import ../../scopes/_firewall.nix;
  secretsScope = import ../../scopes/_secrets.nix;
  backupScope = import ../../scopes/_backup.nix;
  monitoringScope = import ../../scopes/_monitoring.nix;
  coreNixos = ../../core/_nixos.nix;
  agentModule = ../../scopes/nixfleet/_agent.nix;
  controlPlaneModule = ../../scopes/nixfleet/_control-plane.nix;
in {
  # Build a runCommand that prints PASS/FAIL for each assertion and fails on first failure.
  mkEvalCheck = pkgs: name: assertions:
    pkgs.runCommand "eval-test-${name}" {} (
      lib.concatStringsSep "\n" (
        map (a:
          if a.check
          then ''echo "PASS: ${a.msg}"''
          else ''echo "FAIL: ${a.msg}" >&2; exit 1'')
        assertions
      )
      + "\ntouch $out\n"
    );

  # Default hostSpec values for VM test nodes.
  defaultTestSpec = {
    hostName = "testvm";
    userName = "testuser";
    isImpermanent = false;
  };

  # Build a nixosTest-compatible node config with stubbed secrets and known passwords.
  # Returns a NixOS module (attrset) — nixosTest handles calling nixosSystem.
  #
  # Parameters:
  #   inputs          — flake inputs (needs home-manager, nixpkgs, disko, impermanence)
  #   nixosModules    — additional deferred NixOS modules (e.g. agent/CP)
  #   hmModules       — additional deferred HM modules
  #   hmLinuxModules  — Linux-only HM modules (default [])
  #   hostSpecModule  — path to the hostSpec module
  #   hostSpecValues  — hostSpec attrset for this test node
  #   extraModules    — additional NixOS modules (default [])
  mkTestNode = {
    inputs,
    nixosModules ? [],
    hmModules ? [],
    hmLinuxModules ? [],
    hostSpecModule,
  }: {
    hostSpecValues,
    extraModules ? [],
  }: {
    imports =
      [
        hostSpecModule
        {hostSpec = hostSpecValues;}
        # Framework input modules (disko, impermanence) — mkHost injects these,
        # but test nodes need them explicitly
        inputs.disko.nixosModules.disko
        inputs.impermanence.nixosModules.impermanence
        # Framework core + scopes (plain modules, no longer need `inputs` arg)
        coreNixos
        baseScope.nixos
        impermanenceScope.nixos
        firewallScope.nixos
        secretsScope.nixos
        backupScope.nixos
        monitoringScope.nixos
        agentModule
        controlPlaneModule
      ]
      ++ nixosModules
      ++ [
        inputs.home-manager.nixosModules.home-manager
        {
          # --- Test user with known password ---
          users.users.${hostSpecValues.userName} = {
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };
          users.users.root = {
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };

          # --- Handle nixpkgs for test nodes ---
          nixpkgs.pkgs = lib.mkForce (import inputs.nixpkgs {
            system = "x86_64-linux";
            config = {
              allowUnfree = true;
              allowBroken = false;
              allowInsecure = false;
              allowUnsupportedSystem = true;
            };
          });
          nixpkgs.config = lib.mkForce {};
          nixpkgs.hostPlatform = lib.mkForce "x86_64-linux";

          # --- HM config for the test user ---
          home-manager = {
            useGlobalPkgs = true;
            useUserPackages = true;
            users.${hostSpecValues.userName} = {
              imports =
                [hostSpecModule]
                ++ [baseScope.homeManager impermanenceScope.hmLinux]
                ++ hmModules
                ++ hmLinuxModules;
              hostSpec = hostSpecValues;
              home = {
                stateVersion = "24.11";
                username = hostSpecValues.userName;
                homeDirectory = "/home/${hostSpecValues.userName}";
                enableNixpkgsReleaseCheck = false;
              };
              systemd.user.startServices = "sd-switch";
            };
          };
        }
      ]
      ++ extraModules;
  };
}
