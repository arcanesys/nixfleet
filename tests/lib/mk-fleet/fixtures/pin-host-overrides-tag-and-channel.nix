{mkFleet, ...}:
mkFleet {
  hosts = {
    web-01 = {
      system = "x86_64-linux";
      configuration = import ./_stub-configuration.nix {};
      tags = ["infra"];
      channel = "stable";
      # Most-specific level: host pin wins over tag + channel pins below.
      pin = {
        commit = "host-level-abc1234";
        reason = "investigating CVE-2026-9999";
      };
    };
    web-02 = {
      system = "x86_64-linux";
      configuration = import ./_stub-configuration.nix {};
      tags = ["infra"];
      channel = "stable";
      # No host pin → tag-level "infra-freeze" pin applies.
    };
    edge-01 = {
      system = "x86_64-linux";
      configuration = import ./_stub-configuration.nix {};
      tags = [];
      channel = "stable";
      # No host or tag pin → channel-level pin applies.
    };
  };
  tags.infra.pin = {
    commit = "tag-level-def5678";
    reason = "audit-window freeze";
  };
  channels.stable = {
    rolloutPolicy = "default";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
    pin = {
      commit = "channel-level-9876fed";
      reason = "stable lags edge by N commits";
    };
  };
  rolloutPolicies.default = {
    strategy = "canary";
    waves = [{selector.all = true;}];
  };
}
