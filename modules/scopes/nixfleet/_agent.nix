# NixOS service module for the NixFleet fleet agent.
# Auto-included by mkHost (disabled by default).
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

    allowInsecure = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Allow insecure HTTP connections to the control plane. Development only.";
    };

    tls = {
      caCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/etc/nixfleet/fleet-ca.pem";
        description = "Path to CA certificate PEM file for verifying the control plane. Trusted alongside system roots.";
      };

      clientCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/agent-cert.pem";
        description = "Path to client certificate PEM file for mTLS authentication.";
      };

      clientKey = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/agent-key.pem";
        description = "Path to client private key PEM file for mTLS authentication.";
      };
    };

    metricsPort = lib.mkOption {
      type = lib.types.nullOr lib.types.port;
      default = null;
      description = "Port for agent Prometheus metrics. Null disables.";
    };

    metricsOpenFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open agent metrics port in the firewall.";
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

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      environment.etc."nixfleet/health-checks.json".text = builtins.toJSON {
        systemd = cfg.healthChecks.systemd;
        http = cfg.healthChecks.http;
        command = cfg.healthChecks.command;
      };

      systemd.services.nixfleet-agent = {
        description = "NixFleet Fleet Management Agent";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target" "nix-daemon.service"];
        wants = ["network-online.target"];
        startLimitIntervalSec = 0;

        # Agent shells out to nix (copy, path-info) and switch-to-configuration
        path = [config.nix.package pkgs.systemd];

        environment =
          {
            # Nix writes its metadata cache (narinfo lookups, eval cache, etc.)
            # to $XDG_CACHE_HOME (default: ~/.cache). Point it at the agent's
            # StateDirectory so the cache persists on impermanent hosts instead
            # of being wiped on every reboot.
            XDG_CACHE_HOME = "/var/lib/nixfleet/.cache";
          }
          // lib.optionalAttrs (cfg.tags != []) {
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
            ++ lib.optionals cfg.dryRun [
              "--dry-run"
            ]
            ++ lib.optionals cfg.allowInsecure [
              "--allow-insecure"
            ]
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
            ]
          );
          Restart = "always";
          RestartSec = 30;
          StateDirectory = "nixfleet";

          # The agent is a privileged system manager: it runs
          # switch-to-configuration which modifies /boot, /etc, /home, /root,
          # bootloader, kernel, systemd units, etc. Sandboxing blocks these
          # operations (subprocess inherits the agent's namespace).
          # Threat model is equivalent to `sudo nixos-rebuild switch` as a
          # daemon - no sandboxing applied.
          NoNewPrivileges = true;
        };
      };

      # Open metrics port if requested
      networking.firewall.allowedTCPPorts =
        lib.mkIf (cfg.metricsPort != null && cfg.metricsOpenFirewall) [cfg.metricsPort];
    })

    # Impermanence: persist agent state across reboots. Outer mkIf so
    # environment.persistence isn't referenced on non-impermanent hosts.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet"];
    })
  ];
}
