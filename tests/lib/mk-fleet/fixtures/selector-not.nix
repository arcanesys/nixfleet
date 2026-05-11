{mkFleet, ...}: let
  stub = import ./_stub-configuration.nix {};
in
  mkFleet {
    hosts = {
      a = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["web"];
        channel = "stable";
      };
      b = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["web" "deprecated"];
        channel = "stable";
      };
      c = {
        system = "x86_64-linux";
        configuration = stub;
        tags = ["web"];
        channel = "stable";
      };
    };
    channels.stable = {
      rolloutPolicy = "skip-deprecated";
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
    };
    rolloutPolicies.skip-deprecated = {
      strategy = "all-at-once";
      waves = [
        {
          selector.not = {tags = ["deprecated"];};
          soakMinutes = 0;
        }
      ];
    };
  }
