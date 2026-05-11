{mkFleet, ...}:
mkFleet {
  hosts.web-01 = {
    system = "x86_64-linux";
    configuration = import ../fixtures/_stub-configuration.nix {};
    # Carries TWO tags whose pins both apply - eval-time error.
    tags = ["infra" "audit-2026q2"];
    channel = "stable";
  };
  tags = {
    infra.pin = {
      commit = "abc1234";
      reason = "infra freeze";
    };
    audit-2026q2.pin = {
      commit = "def5678";
      reason = "Q2 audit window";
    };
  };
  channels.stable = {
    rolloutPolicy = "default";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
  };
  rolloutPolicies.default = {
    strategy = "canary";
    waves = [{selector.all = true;}];
  };
}
