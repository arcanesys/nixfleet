{mkFleet, ...}:
mkFleet {
  hosts.m = {
    system = "x86_64-linux";
    configuration = import ../fixtures/_stub-configuration.nix {};
    tags = [];
    channel = "stable";
  };
  channels.stable = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
    compliance.frameworks = ["fictional-framework/v99"];
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
