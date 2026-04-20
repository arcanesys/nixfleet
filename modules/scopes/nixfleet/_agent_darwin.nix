# Darwin service module for the NixFleet fleet agent.
# Auto-included by mkHost for Darwin hosts (disabled by default).
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;
  nixfleet-agent = pkgs.callPackage ../../../crates/agent {inherit inputs;};
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
      default = config.hostSpec.hostName;
      defaultText = lib.literalExpression "config.hostSpec.hostName";
      description = "Machine identifier reported to the control plane.";
    };

    pollInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Poll interval in seconds (steady-state).";
    };

    retryInterval = lib.mkOption {
      type = lib.types.int;
      default = 30;
      description = "Retry interval in seconds after a failed poll.";
    };

    cacheUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "https://cache.fleet.example.com";
      description = "Binary cache URL for nix copy --from.";
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

    allowInsecure = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Allow insecure HTTP connections to the control plane.";
    };

    tls = {
      caCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to CA certificate PEM for verifying the control plane.";
      };

      clientCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to client certificate PEM file for mTLS.";
      };

      clientKey = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to client private key PEM file for mTLS.";
      };
    };

    metricsPort = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default = null;
      description = "Port for agent Prometheus metrics. Null disables.";
    };

    tags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Tags for grouping this machine in fleet operations.";
    };

    healthInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Seconds between continuous health reports.";
    };

    healthChecks = {
      launchd = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options.labels = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            description = "Launchd service labels that must be running.";
          };
        });
        default = [];
        description = "Launchd service health checks.";
      };

      http = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options = {
            url = lib.mkOption {
              type = lib.types.str;
              description = "URL to GET.";
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
    # Write health check config for the agent to read at runtime
    environment.etc."nixfleet/health-checks.json".text = builtins.toJSON {
      launchd = cfg.healthChecks.launchd;
      http = cfg.healthChecks.http;
      command = cfg.healthChecks.command;
    };

    # Ensure state directory exists before launchd tries to start the agent.
    # nix-darwin uses preActivation/postActivation, not named scripts like NixOS.
    system.activationScripts.preActivation.text = ''
      mkdir -p /var/lib/nixfleet
    '';

    # Force-restart the agent after every activation. nix-darwin's launchd
    # management only reloads services whose plist changed. When the plist
    # is unchanged (same agent binary + same config), the agent process
    # can still be killed during activation (profile switch), and launchd's
    # KeepAlive doesn't reliably restart it in the transient state. This
    # ensures the agent is always running after activation completes.
    system.activationScripts.postActivation.text = lib.mkAfter ''
      echo "restarting nixfleet agent..." >&2
      launchctl kickstart -k system/com.nixfleet.agent 2>/dev/null || true
    '';

    launchd.daemons.nixfleet-agent = {
      serviceConfig = {
        Label = "com.nixfleet.agent";
        # Wrapped in sh -c with a 15s delay. At boot, launchd starts
        # the agent before NTP syncs the clock and before agenix
        # decrypts mTLS certs. The delay gives both time to complete.
        # The exec replaces sh so launchd tracks the agent PID directly.
        ProgramArguments = let
          args = lib.concatStringsSep " " ([
              "${nixfleet-agent}/bin/nixfleet-agent"
              "--control-plane-url"
              (lib.escapeShellArg cfg.controlPlaneUrl)
              "--machine-id"
              (lib.escapeShellArg cfg.machineId)
              "--poll-interval"
              (toString cfg.pollInterval)
              "--retry-interval"
              (toString cfg.retryInterval)
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
            ++ lib.optionals cfg.dryRun ["--dry-run"]
            ++ lib.optionals cfg.allowInsecure ["--allow-insecure"]
            ++ lib.optionals (cfg.tls.caCert != null) [
              "--ca-cert"
              (lib.escapeShellArg cfg.tls.caCert)
            ]
            ++ lib.optionals (cfg.tls.clientCert != null) [
              "--client-cert"
              (lib.escapeShellArg cfg.tls.clientCert)
            ]
            ++ lib.optionals (cfg.tls.clientKey != null) [
              "--client-key"
              (lib.escapeShellArg cfg.tls.clientKey)
            ]
            ++ lib.optionals (cfg.metricsPort != null) [
              "--metrics-port"
              (toString cfg.metricsPort)
            ]);
        in ["/bin/sh" "-c" "sleep 15 && exec ${args}"];
        KeepAlive = true;
        RunAtLoad = true;
        ThrottleInterval = 10;
        ExitTimeOut = 10;
        StandardOutPath = "/var/log/nixfleet-agent.log";
        StandardErrorPath = "/var/log/nixfleet-agent.log";
        WorkingDirectory = "/var/lib/nixfleet";
        EnvironmentVariables =
          {
            XDG_CACHE_HOME = "/var/lib/nixfleet/.cache";
            # Agent shells out to nix (copy, path-info) and nix-env.
            # Launchd daemons have a minimal PATH - add nix paths explicitly.
            PATH = "/nix/var/nix/profiles/default/bin:/run/current-system/sw/bin:/usr/bin:/bin";
          }
          // lib.optionalAttrs (cfg.tags != []) {
            NIXFLEET_TAGS = lib.concatStringsSep "," cfg.tags;
          };
      };
    };
  };
}
