# Example: Batch host declaration from a template.
#
# Standard Nix — no framework function needed.
# 50 identical edge devices + named hosts, all in one flake output.
{config, ...}: let
  mkHost = config.flake.lib.mkHost;

  # Shared defaults
  acme = {
    userName = "deploy";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
  };

  # Batch: 50 edge devices from a template
  edgeHosts = builtins.listToAttrs (map (i: {
    name = "edge-${toString i}";
    value = mkHost {
      hostName = "edge-${toString i}";
      platform = "aarch64-linux";
      hostSpec =
        acme
        // {
          isMinimal = true;
          isServer = true;
        };
      modules = [
        # ./hosts/edge/common-hardware.nix
        # ./hosts/edge/disk-config.nix
      ];
    };
  }) (builtins.genList (i: i + 1) 50));

  # Named hosts
  namedHosts = {
    control-plane = mkHost {
      hostName = "control-plane";
      platform = "x86_64-linux";
      hostSpec =
        acme
        // {
          isServer = true;
        };
      modules = [
        # ./hosts/control-plane/hardware.nix
        # ./hosts/control-plane/disk-config.nix
      ];
    };
  };
in {
  # Merge named hosts + batch hosts into one output
  flake.nixosConfigurations = namedHosts // edgeHosts;
}
