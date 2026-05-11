# Channels strict-merge; rolloutPolicies follow later-wins.
{mergeFleets, ...}: let
  stub = import ../fixtures/_stub-configuration.nix {};
  fleetWithChannel = hostName: freshness: {
    hosts.${hostName} = {
      system = "x86_64-linux";
      configuration = stub;
      tags = [];
      channel = "stable";
    };
    channels.stable = {
      rolloutPolicy = "all";
      signingIntervalMinutes = 60;
      freshnessWindow = freshness;
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
    (fleetWithChannel "a" 180)
    (fleetWithChannel "b" 240)
  ]
