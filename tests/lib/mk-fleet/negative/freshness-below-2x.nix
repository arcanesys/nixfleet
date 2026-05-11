{mkFleet, ...}: let
  stub = import ../fixtures/_stub-configuration.nix {};
in
  mkFleet {
    hosts.m = {
      system = "x86_64-linux";
      configuration = stub;
      tags = [];
      channel = "stable";
    };
    channels.stable = {
      rolloutPolicy = "all-at-once";
      signingIntervalMinutes = 60;
      freshnessWindow = 90; # < 2 × 60, must fail
    };
    rolloutPolicies.all-at-once = {
      strategy = "all-at-once";
      waves = [
        {
          selector.all = true;
          soakMinutes = 0;
        }
      ];
    };
  }
