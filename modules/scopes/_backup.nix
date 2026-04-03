# Backup scaffolding — backend-agnostic timer, hooks, health, persistence.
# Fleet repos set systemd.services.nixfleet-backup.serviceConfig.ExecStart
# to their chosen backup tool (restic, borgbackup, etc.).
# Returns { nixos } module attrset.
# mkHost imports this directly; it activates via nixfleet.backup.enable.
{
  nixos = {
    config,
    lib,
    pkgs,
    ...
  }: let
    hS = config.hostSpec;
    cfg = config.nixfleet.backup;
    types = lib.types;
  in {
    options.nixfleet.backup = {
      enable = lib.mkEnableOption "NixFleet backup scaffolding (timer, health, persistence)";

      paths = lib.mkOption {
        type = types.listOf types.str;
        default = ["/persist"];
        description = "Directories to back up.";
      };

      exclude = lib.mkOption {
        type = types.listOf types.str;
        default = ["/persist/nix" "*.cache"];
        description = "Patterns to exclude from backup.";
      };

      schedule = lib.mkOption {
        type = types.str;
        default = "daily";
        description = "Systemd calendar expression (daily, weekly, *-*-* 02:00:00).";
      };

      retention = lib.mkOption {
        type = types.submodule {
          options = {
            daily = lib.mkOption {
              type = types.int;
              default = 7;
              description = "Number of daily snapshots to keep.";
            };
            weekly = lib.mkOption {
              type = types.int;
              default = 4;
              description = "Number of weekly snapshots to keep.";
            };
            monthly = lib.mkOption {
              type = types.int;
              default = 6;
              description = "Number of monthly snapshots to keep.";
            };
          };
        };
        default = {};
        description = "Retention policy. Interpretation depends on fleet-chosen backend.";
      };

      healthCheck = {
        onSuccess = lib.mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "https://hc-ping.com/xxx";
          description = "URL to GET on successful backup.";
        };
        onFailure = lib.mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "URL to GET on backup failure.";
        };
      };

      preHook = lib.mkOption {
        type = types.lines;
        default = "";
        description = "Shell commands to run before backup.";
      };

      postHook = lib.mkOption {
        type = types.lines;
        default = "";
        description = "Shell commands to run after successful backup.";
      };

      stateDirectory = lib.mkOption {
        type = types.str;
        default = "/var/lib/nixfleet-backup";
        description = "Directory for backup state/cache. Persisted on impermanent hosts.";
      };
    };

    config = lib.mkIf cfg.enable {
      # Systemd timer with staggered delay across fleet
      systemd.timers.nixfleet-backup = {
        wantedBy = ["timers.target"];
        timerConfig = {
          OnCalendar = cfg.schedule;
          Persistent = true;
          RandomizedDelaySec = "1h";
        };
      };

      # Service skeleton — fleet module sets ExecStart
      systemd.services.nixfleet-backup = {
        description = "NixFleet Backup";
        after = ["network-online.target"];
        wants = ["network-online.target"];

        serviceConfig = {
          Type = "oneshot";
          StateDirectory = "nixfleet-backup";
        };

        # Pre-hook
        preStart = lib.mkIf (cfg.preHook != "") cfg.preHook;

        # Post-hook + health ping + status reporting
        postStart = let
          postHookCmd = lib.optionalString (cfg.postHook != "") cfg.postHook;
          healthCmd =
            lib.optionalString (cfg.healthCheck.onSuccess != null)
            "${pkgs.curl}/bin/curl -fsS -m 10 --retry 3 ${lib.escapeShellArg cfg.healthCheck.onSuccess} || true";
          statusCmd = ''
            cat > ${cfg.stateDirectory}/status.json <<STATUSEOF
            {"lastRun": "$(date -Is)", "status": "success", "hostname": "${config.networking.hostName}"}
            STATUSEOF
          '';
        in
          lib.concatStringsSep "\n" (lib.filter (s: s != "") [postHookCmd healthCmd statusCmd]);
      };

      # On-failure notification service
      systemd.services.nixfleet-backup-failure = lib.mkIf (cfg.healthCheck.onFailure != null) {
        description = "NixFleet Backup Failure Notification";
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${pkgs.curl}/bin/curl -fsS -m 10 --retry 3 ${lib.escapeShellArg cfg.healthCheck.onFailure}";
        };
      };
      systemd.services.nixfleet-backup.unitConfig.OnFailure =
        lib.mkIf (cfg.healthCheck.onFailure != null) ["nixfleet-backup-failure.service"];

      # Impermanence: persist backup state
      environment.persistence."/persist".directories =
        lib.mkIf (hS.isImpermanent or false) [cfg.stateDirectory];
    };
  };
}
