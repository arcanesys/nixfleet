{mergeFleets, ...}: let
  stub = import ../fixtures/_stub-configuration.nix {};
  shared = hostTags: {
    hosts.duplicated = {
      system = "x86_64-linux";
      configuration = stub;
      tags = hostTags;
      channel = "stable";
    };
    channels.stable = {
      rolloutPolicy = "all";
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
    };
    rolloutPolicies.all = {
      strategy = "all-at-once";
      waves = [
        {
          selector.all = true;
          soakMinutes = 0;
        }
      ];
    };
  };
in
  mergeFleets [
    (shared ["a"])
    (shared ["b"])
  ]
