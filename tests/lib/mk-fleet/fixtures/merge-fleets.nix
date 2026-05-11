{mergeFleets, ...}: let
  stub = import ./_stub-configuration.nix {};
  paris = {
    hosts.paris-1 = {
      system = "x86_64-linux";
      configuration = stub;
      tags = ["paris" "eu-fr"];
      channel = "stable";
    };
    tags.paris.description = "Paris datacenter.";
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
    disruptionBudgets = [
      {
        selector.tags = ["paris"];
        maxInFlight = 1;
      }
    ];
  };
  lyon = {
    hosts.lyon-1 = {
      system = "x86_64-linux";
      configuration = stub;
      tags = ["lyon" "eu-fr"];
      channel = "edge";
    };
    tags.lyon.description = "Lyon datacenter.";
    channels.edge = {
      rolloutPolicy = "all";
      signingIntervalMinutes = 60;
      freshnessWindow = 240;
    };
    disruptionBudgets = [
      {
        selector.tags = ["lyon"];
        maxInFlight = 1;
      }
    ];
  };
in
  mergeFleets [paris lyon]
