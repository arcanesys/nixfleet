# Backup scaffolding — backend-agnostic timer, hooks, health, persistence.
# Optional concrete backends: restic, borgbackup.
# When backend is null, fleet repos set systemd.services.nixfleet-backup.serviceConfig.ExecStart
# to their chosen backup tool.
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

    excludeFlags =
      lib.concatMapStringsSep " " (p: "--exclude ${lib.escapeShellArg p}") cfg.exclude;

    resticBackupScript = pkgs.writeShellScript "nixfleet-backup-restic" ''
      set -euo pipefail
      export RESTIC_REPOSITORY=${lib.escapeShellArg cfg.restic.repository}
      export RESTIC_PASSWORD_FILE=${lib.escapeShellArg cfg.restic.passwordFile}
      export RESTIC_CACHE_DIR=${cfg.stateDirectory}/restic-cache

      # Initialize repo if needed (idempotent)
      ${pkgs.restic}/bin/restic cat config >/dev/null 2>&1 || \
        ${pkgs.restic}/bin/restic init

      # Backup
      ${pkgs.restic}/bin/restic backup \
        --tag ${lib.escapeShellArg config.hostSpec.hostName} \
        ${excludeFlags} \
        ${lib.concatStringsSep " " (map lib.escapeShellArg cfg.paths)}

      # Prune
      ${pkgs.restic}/bin/restic forget \
        --keep-daily ${toString cfg.retention.daily} \
        --keep-weekly ${toString cfg.retention.weekly} \
        --keep-monthly ${toString cfg.retention.monthly} \
        --prune
    '';

    borgArchiveName = "${config.hostSpec.hostName}-{now:%Y-%m-%dT%H:%M:%S}";

    borgBackupScript = pkgs.writeShellScript "nixfleet-backup-borg" ''
      set -euo pipefail
      export BORG_REPO=${lib.escapeShellArg cfg.borgbackup.repository}
      ${lib.optionalString (cfg.borgbackup.passphraseFile != null)
        "export BORG_PASSCOMMAND=\"cat ${lib.escapeShellArg cfg.borgbackup.passphraseFile}\""}
      ${lib.optionalString (cfg.borgbackup.passphraseFile == null)
        "export BORG_PASSPHRASE=\"\""}

      # Initialize repo if needed (idempotent)
      ${pkgs.borgbackup}/bin/borg info "$BORG_REPO" >/dev/null 2>&1 || \
        ${pkgs.borgbackup}/bin/borg init --encryption=${lib.escapeShellArg cfg.borgbackup.encryption}

      # Backup
      ${pkgs.borgbackup}/bin/borg create \
        ${excludeFlags} \
        "$BORG_REPO::${borgArchiveName}" \
        ${lib.concatStringsSep " " (map lib.escapeShellArg cfg.paths)}

      # Prune
      ${pkgs.borgbackup}/bin/borg prune \
        --keep-daily ${toString cfg.retention.daily} \
        --keep-weekly ${toString cfg.retention.weekly} \
        --keep-monthly ${toString cfg.retention.monthly}

      ${pkgs.borgbackup}/bin/borg compact
    '';
  in {
    options.nixfleet.backup = {
      enable = lib.mkEnableOption "NixFleet backup scaffolding (timer, health, persistence)";

      backend = lib.mkOption {
        type = types.nullOr (types.enum ["restic" "borgbackup"]);
        default = null;
        description = "Backup backend. Null = fleet sets ExecStart manually.";
      };

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

      restic = {
        repository = lib.mkOption {
          type = types.str;
          default = "";
          example = "/mnt/backup/restic";
          description = "Restic repository URL or path.";
        };
        passwordFile = lib.mkOption {
          type = types.str;
          default = "";
          example = "/run/secrets/restic-password";
          description = "Path to file containing the repository password.";
        };
      };

      borgbackup = {
        repository = lib.mkOption {
          type = types.str;
          default = "";
          example = "/mnt/backup/borg";
          description = "Borg repository path or ssh://user@host/path.";
        };
        passphraseFile = lib.mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Path to file containing the repository passphrase. Null = repokey without passphrase.";
        };
        encryption = lib.mkOption {
          type = types.str;
          default = "repokey";
          description = "Borg encryption mode (repokey, repokey-blake2, none, etc.).";
        };
      };
    };

    config = lib.mkIf cfg.enable {
      # Fail early at eval time if the selected backend's required
      # fields are left at their empty defaults. A runtime failure
      # from restic/borg would be harder to diagnose.
      assertions = [
        {
          assertion = cfg.backend != "restic" || (cfg.restic.repository != "" && cfg.restic.passwordFile != "");
          message = "nixfleet.backup: restic backend requires restic.repository and restic.passwordFile";
        }
        {
          assertion = cfg.backend != "borgbackup" || cfg.borgbackup.repository != "";
          message = "nixfleet.backup: borgbackup backend requires borgbackup.repository";
        }
      ];

      # Systemd timer with staggered delay across fleet
      systemd.timers.nixfleet-backup = {
        wantedBy = ["timers.target"];
        timerConfig = {
          OnCalendar = cfg.schedule;
          Persistent = true;
          RandomizedDelaySec = "1h";
        };
      };

      # Service skeleton — backend sets ExecStart, or fleet module overrides
      systemd.services.nixfleet-backup = {
        description = "NixFleet Backup";
        after = ["network-online.target"];
        wants = ["network-online.target"];

        serviceConfig = lib.mkMerge [
          {
            Type = "oneshot";
            StateDirectory = "nixfleet-backup";
          }
          (lib.mkIf (cfg.backend == "restic") {
            ExecStart = resticBackupScript;
          })
          (lib.mkIf (cfg.backend == "borgbackup") {
            ExecStart = borgBackupScript;
          })
        ];

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
            {"lastRun": "$(date -Is)", "status": "success", "hostname": "${config.hostSpec.hostName}"}
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

      # Add backend packages to system
      environment.systemPackages =
        lib.optional (cfg.backend == "restic") pkgs.restic
        ++ lib.optional (cfg.backend == "borgbackup") pkgs.borgbackup;

      # Impermanence: persist backup state
      environment.persistence."/persist".directories =
        lib.mkIf (hS.isImpermanent or false) [cfg.stateDirectory];
    };
  };
}
