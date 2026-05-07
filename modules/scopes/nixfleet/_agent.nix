{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;
  nixfleet-agent = cfg.package;

  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;

  # Issue #86: per-host probe declaration is too rich for CLI flags
  # (variable lists of nested objects), so we materialise the
  # `services.nixfleet-agent.healthChecks` value to a JSON file in
  # /etc/ and pass `--health-checks-config <path>` instead. Mirrors
  # the trust.json convention.
  healthChecksConfig = {
    inherit (cfg.healthChecks) mode http tcp exec;
  };
  healthChecksJson = pkgs.writers.writeJSON "health-checks.json" healthChecksConfig;
  hasHealthChecks =
    cfg.healthChecks.http != []
    || cfg.healthChecks.tcp != []
    || cfg.healthChecks.exec != [];
in {
  imports = [./_agent-options.nix];

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      environment.etc."nixfleet/agent/trust.json".source = trustJson;
      environment.etc."nixfleet/agent/health-checks.json" = lib.mkIf hasHealthChecks {
        source = healthChecksJson;
      };

      systemd.services.nixfleet-agent = {
        description = "NixFleet Fleet Management Agent";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target" "nix-daemon.service"];
        wants = ["network-online.target"];
        startLimitIntervalSec = 0;

        # LOADBEARING: agent reads /etc/nixfleet/agent/{trust,health-checks}.json
        # ONCE at startup. Without restartTriggers a content-only change to
        # those files (most common: operator edits healthChecks in fleet.nix
        # → CI rolls a new closure with a new JSON derivation; ExecStart
        # line is unchanged because the path is constant) leaves the agent
        # running with stale config until something else triggers a
        # restart. Pinning the JSON derivations to restartTriggers makes
        # `switch-to-configuration` see a content delta and restart the
        # unit at activation, so declarative edits land deterministically.
        restartTriggers =
          [trustJson]
          ++ lib.optional hasHealthChecks healthChecksJson;

        # LOADBEARING: bypasses `nixos-rebuild`; closure pre-built, we shell out to nix-store/switch-to-configuration to dodge rebuild-ng CLI churn.
        path = [config.nix.package pkgs.systemd];

        environment =
          {
            # Survive impermanence: keep the narinfo/eval cache in StateDirectory.
            XDG_CACHE_HOME = "/var/lib/nixfleet/.cache";
          }
          // lib.optionalAttrs (cfg.tags != []) {
            NIXFLEET_TAGS = lib.concatStringsSep "," cfg.tags;
          };

        serviceConfig = {
          Type = "simple";
          ExecStart = lib.concatStringsSep " " (import ./_agent-args.nix {
            inherit lib cfg;
            package = nixfleet-agent;
          });
          Restart = "always";
          RestartSec = 30;
          # GOTCHA: StateDirectory= is relative to /var/lib; basename only.
          StateDirectory =
            if lib.hasPrefix "/var/lib/" cfg.stateDir
            then lib.removePrefix "/var/lib/" cfg.stateDir
            else "nixfleet-agent";

          # FOOTGUN: no sandboxing; agent runs switch-to-configuration mutating /boot, /etc, bootloader, kernel — equivalent to `sudo nixos-rebuild switch` as a daemon.
          NoNewPrivileges = true;
        };
      };
    })

    (lib.mkIf cfg.enable {
      nixfleet.persistence.directories = ["/var/lib/nixfleet"];
    })
  ];
}
