{mkFleet, ...}: let
  stub = import ../fixtures/_stub-configuration.nix {};
in
  mkFleet {
    hosts = {
      a = {
        system = "x86_64-linux";
        configuration = stub;
        tags = [];
        channel = "stable";
      };
      b = {
        system = "x86_64-linux";
        configuration = stub;
        tags = [];
        channel = "stable";
      };
    };
    channels.stable = {
      rolloutPolicy = "all-at-once";
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
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
    edges = [
      {
        gated = "a";
        gates = "b";
        reason = "a waits for b";
      }
      {
        gated = "b";
        gates = "a";
        reason = "cycle!";
      }
    ];
  }
