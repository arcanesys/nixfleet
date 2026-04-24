{
  nixfleet,
  self,
  ...
}:
nixfleet.lib.mkFleet {
  hosts = {
    m70q-attic = {
      system = "x86_64-linux";
      configuration = self.nixosConfigurations.m70q-attic;
      tags = ["homelab" "always-on" "eu-fr" "server" "coordinator"];
      channel = "stable";
    };

    workstation = {
      system = "x86_64-linux";
      configuration = self.nixosConfigurations.workstation;
      tags = ["homelab" "eu-fr" "workstation" "builder"];
      channel = "stable";
    };

    rpi-sensor-01 = {
      system = "aarch64-linux";
      configuration = self.nixosConfigurations.rpi-sensor-01;
      tags = ["edge" "eu-fr" "sensor" "low-power"];
      channel = "edge-slow";
    };
  };

  tags = {
    homelab.description = "Personal fleet.";
    "always-on".description = "24/7 availability expected.";
    "eu-fr".description = "France-resident; ANSSI BP-028 applies.";
    coordinator.description = "Hosts Forgejo, attic cache, reconciler.";
    builder.description = "Enough RAM/CPU to build closures.";
  };

  channels = {
    stable = {
      description = "Workstation canary → M70q promote.";
      rolloutPolicy = "homelab-canary";
      reconcileIntervalMinutes = 30;
      compliance = {
        strict = true;
        frameworks = ["anssi-bp028"];
      };
    };
    edge-slow = {
      description = "Low-power sensors; weekly reconcile.";
      rolloutPolicy = "all-at-once";
      reconcileIntervalMinutes = 10080;
    };
  };

  rolloutPolicies = {
    homelab-canary = {
      strategy = "canary";
      waves = [
        {
          selector = {tags = ["workstation"];};
          soakMinutes = 30;
        }
        {
          selector = {tags = ["always-on"];};
          soakMinutes = 60;
        }
      ];
      healthGate = {systemdFailedUnits.max = 0;};
      onHealthFailure = "rollback-and-halt";
    };
    all-at-once = {
      strategy = "all-at-once";
      waves = [
        {
          selector = {all = true;};
          soakMinutes = 0;
        }
      ];
      healthGate = {systemdFailedUnits.max = 0;};
    };
  };

  # Edges are for upgrade ordering (e.g. schema migrations), not runtime
  # dependencies. Workstation depends on M70q's attic cache at fetch time,
  # but the cache serves all generations — no ordering constraint.
  edges = [];

  disruptionBudgets = [
    {
      selector = {tags = ["always-on"];};
      maxInFlight = 1;
    }
    {
      selector = {tags = ["coordinator"];};
      maxInFlight = 1;
    }
  ];
}
