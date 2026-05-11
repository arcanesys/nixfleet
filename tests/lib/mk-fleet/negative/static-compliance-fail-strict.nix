{mkFleet, ...}:
mkFleet {
  hosts.m = {
    system = "x86_64-linux";
    configuration = {
      config = {
        system.build.toplevel = {
          outPath = "/nix/store/0000000000000000000000000000000000000000-stub";
          drvPath = "/nix/store/0000000000000000000000000000000000000000-stub.drv";
        };
        compliance.evidence.probes = {
          accessControl = {
            type = "static";
            staticEvidence = {
              passed = false;
              evidence = {
                sshPasswordAuthDisabled = false;
              };
            };
          };
        };
      };
    };
    tags = [];
    channel = "prod";
  };
  channels.prod = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
    compliance = {
      mode = "enforce";
      frameworks = [];
    };
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
