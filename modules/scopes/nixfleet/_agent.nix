# NixOS service module for the NixFleet fleet agent.
# Auto-included by mkHost (disabled by default).
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;
  nixfleet-agent = pkgs.callPackage ../../../agent {};
in {
  options.services.nixfleet-agent = {
    enable = lib.mkEnableOption "NixFleet fleet management agent";

    controlPlaneUrl = lib.mkOption {
      type = lib.types.str;
      example = "https://fleet.example.com";
      description = "URL of the NixFleet control plane.";
    };

    machineId = lib.mkOption {
      type = lib.types.str;
      default = config.networking.hostName;
      defaultText = lib.literalExpression "config.networking.hostName";
      description = "Machine identifier reported to the control plane.";
    };

    pollInterval = lib.mkOption {
      type = lib.types.int;
      default = 300;
      description = "Poll interval in seconds.";
    };

    cacheUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "https://cache.fleet.example.com";
      description = "Binary cache URL for nix copy --from. Falls back to control plane default.";
    };

    dbPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet/state.db";
      description = "Path to the SQLite state database.";
    };

    dryRun = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "When true, check and fetch but do not apply generations.";
    };

    tags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Tags for grouping this machine in fleet operations.";
    };

    healthInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Seconds between continuous health reports to control plane.";
    };

    healthChecks = {
      systemd = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options.units = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            description = "Systemd units that must be active.";
          };
        });
        default = [];
        description = "Systemd unit health checks.";
      };

      http = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options = {
            url = lib.mkOption {
              type = lib.types.str;
              description = "URL to GET.";
            };
            interval = lib.mkOption {
              type = lib.types.int;
              default = 5;
              description = "Check interval in seconds.";
            };
            timeout = lib.mkOption {
              type = lib.types.int;
              default = 3;
              description = "Timeout in seconds.";
            };
            expectedStatus = lib.mkOption {
              type = lib.types.int;
              default = 200;
              description = "Expected HTTP status code.";
            };
          };
        });
        default = [];
        description = "HTTP endpoint health checks.";
      };

      command = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options = {
            name = lib.mkOption {
              type = lib.types.str;
              description = "Check name.";
            };
            command = lib.mkOption {
              type = lib.types.str;
              description = "Shell command (exit 0 = healthy).";
            };
            interval = lib.mkOption {
              type = lib.types.int;
              default = 10;
              description = "Check interval in seconds.";
            };
            timeout = lib.mkOption {
              type = lib.types.int;
              default = 5;
              description = "Timeout in seconds.";
            };
          };
        });
        default = [];
        description = "Custom command health checks.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."nixfleet/health-checks.json".text = builtins.toJSON {
      systemd = cfg.healthChecks.systemd;
      http = cfg.healthChecks.http;
      command = cfg.healthChecks.command;
    };

    systemd.services.nixfleet-agent = {
      description = "NixFleet Fleet Management Agent";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      environment = lib.mkIf (cfg.tags != []) {
        NIXFLEET_TAGS = lib.concatStringsSep "," cfg.tags;
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = lib.concatStringsSep " " (
          [
            "${nixfleet-agent}/bin/nixfleet-agent"
            "--control-plane-url"
            (lib.escapeShellArg cfg.controlPlaneUrl)
            "--machine-id"
            (lib.escapeShellArg cfg.machineId)
            "--poll-interval"
            (toString cfg.pollInterval)
            "--db-path"
            (lib.escapeShellArg cfg.dbPath)
            "--health-config"
            "/etc/nixfleet/health-checks.json"
            "--health-interval"
            (toString cfg.healthInterval)
          ]
          ++ lib.optionals (cfg.cacheUrl != null) [
            "--cache-url"
            (lib.escapeShellArg cfg.cacheUrl)
          ]
          ++ lib.optionals cfg.dryRun [
            "--dry-run"
          ]
        );
        Restart = "always";
        RestartSec = 30;
        StateDirectory = "nixfleet";

        # Hardening
        NoNewPrivileges = true;
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ReadWritePaths = ["/var/lib/nixfleet" "/nix/var/nix"];
        ReadOnlyPaths = ["/nix/store" "/run/current-system"];
      };
    };

    # Impermanence: persist agent state across reboots
    environment.persistence."/persist".directories =
      lib.mkIf
      (config.hostSpec.isImpermanent or false)
      ["/var/lib/nixfleet"];
  };
}
