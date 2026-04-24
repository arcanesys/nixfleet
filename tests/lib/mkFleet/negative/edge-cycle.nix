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
        after = "a";
        before = "b";
        reason = "a before b";
      }
      {
        after = "b";
        before = "a";
        reason = "cycle!";
      }
    ];
  }
