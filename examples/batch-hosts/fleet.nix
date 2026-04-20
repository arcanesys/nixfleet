# Example: Batch host declaration from a template.
#
# Standard Nix - no framework function needed.
# 50 identical edge devices + named hosts, all in one flake output.
#
# Operators scope provides a shared user inventory across all machines.
# Roles replace the old posture flags (isServer, isMinimal, etc.).
{
  config,
  inputs,
  ...
}: let
  mkHost = config.flake.lib.mkHost;

  # Shared defaults
  acme = {
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
  };

  # Shared operators config - deployed identically on every host
  operatorsModule = {
    nixfleet.operators = {
      primaryUser = "deploy";
      rootSshKeys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... deploy@acme"
      ];
      users.deploy = {
        isAdmin = true;
        homeManager.enable = false;
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... deploy@acme"
        ];
      };
    };
  };

  # Batch: 50 edge devices from a template
  edgeHosts = builtins.listToAttrs (map (i: {
      name = "edge-${toString i}";
      value = mkHost {
        hostName = "edge-${toString i}";
        platform = "aarch64-linux";
        hostSpec = acme;
        modules = [
          inputs.nixfleet.scopes.roles.server
          operatorsModule
          # ./hosts/edge/common-hardware.nix
          # ./hosts/edge/disk-config.nix
        ];
      };
    })
    (builtins.genList (i: i + 1) 50));

  # Named hosts
  namedHosts = {
    control-plane = mkHost {
      hostName = "control-plane";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [
        inputs.nixfleet.scopes.roles.server
        operatorsModule
        # ./hosts/control-plane/hardware.nix
        # ./hosts/control-plane/disk-config.nix
      ];
    };
  };
in {
  # Merge named hosts + batch hosts into one output
  flake.nixosConfigurations = namedHosts // edgeHosts;
}
