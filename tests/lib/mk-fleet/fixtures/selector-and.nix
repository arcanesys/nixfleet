{mkFleet, ...}: let
  stub = import ./_stub-configuration.nix {};
in
  mkFleet {
    hosts = {
      eu-server = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["eu-fr" "server"];
        channel = "stable";
      };
      eu-workstation = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["eu-fr" "workstation"];
        channel = "stable";
      };
      us-server = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["us-east" "server"];
        channel = "stable";
      };
      sensor = {
        system = "aarch64-linux";
        configuration = stub;
        tags = ["eu-fr" "sensor"];
        channel = "stable";
      };
    };
    channels.stable = {
      rolloutPolicy = "eu-servers-only";
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
    };
    rolloutPolicies.eu-servers-only = {
      strategy = "all-at-once";
      waves = [
        {
          selector.and = [
            {tags = ["eu-fr"];}
            {tags = ["server"];}
          ];
          soakMinutes = 0;
        }
      ];
    };
  }
