{
  lib,
  mkFleet,
  ...
}: let
  stub = import ./_stub-configuration.nix {};
  mkStubHost = tag: {
    system = "x86_64-linux";
    configuration = stub;
    tags = [tag];
    channel = "stable";
  };
in
  mkFleet {
    hosts = lib.genAttrs (map (n: "host-${toString n}") (lib.range 1 10)) (_: mkStubHost "etcd");
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
    disruptionBudgets = [
      {
        selector.tags = ["etcd"];
        maxInFlight = 1;
      }
    ];
  }
