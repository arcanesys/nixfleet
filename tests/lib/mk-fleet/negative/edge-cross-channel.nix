# mkFleet must reject host edges whose two endpoints are on different
# channels. The runtime gate (`gates::host_edges`) silently no-ops on
# such edges to avoid bricking the gated host, but eval-time validation
# fails loud - operators should fix the typo before the artifact even
# gets signed.
{mkFleet, ...}: let
  stub = import ../fixtures/_stub-configuration.nix {};
in
  mkFleet {
    hosts = {
      host-05 = {
        system = "x86_64-linux";
        configuration = stub;
        tags = [];
        channel = "edge";
      };
      host-01 = {
        system = "x86_64-linux";
        configuration = stub;
        tags = [];
        channel = "stable";
      };
    };
    channels = {
      edge = {
        rolloutPolicy = "canary";
        signingIntervalMinutes = 30;
        freshnessWindow = 720;
      };
      stable = {
        rolloutPolicy = "canary";
        signingIntervalMinutes = 30;
        freshnessWindow = 720;
      };
    };
    rolloutPolicies.canary = {
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
        gated = "host-01";
        gates = "host-05";
        reason = "cross-channel - should be rejected";
      }
    ];
  }
