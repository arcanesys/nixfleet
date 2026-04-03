# Example: Acme Corp fleet using NixFleet framework
{config, ...}: let
  mkHost = config.flake.lib.mkHost;

  # Organization defaults (replaces mkOrg)
  acme = {
    userName = "deploy";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
  };
in {
  flake.nixosConfigurations = {
    # Developer workstation
    dev-01 = mkHost {
      hostName = "dev-01";
      platform = "x86_64-linux";
      hostSpec =
        acme
        // {
          isImpermanent = true;
        };
      modules = [
        # ./hosts/dev-01/hardware.nix
        # ./hosts/dev-01/disk-config.nix
      ];
    };

    # Production server
    prod-web-01 = mkHost {
      hostName = "prod-web-01";
      platform = "x86_64-linux";
      hostSpec =
        acme
        // {
          isServer = true;
          isMinimal = true;
        };
      modules = [
        # ./hosts/prod-web-01/hardware.nix
        # ./hosts/prod-web-01/disk-config.nix
      ];
    };
  };
}
