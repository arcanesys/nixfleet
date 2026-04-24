# NixOS service module for the NixFleet fleet agent (v0.2 contract).
#
# Linux-only. Poll-only agent that reads a trust-root declaration from
# /etc/nixfleet/agent/trust.json and talks to the control plane over
# mTLS. Reload model is restart-only (docs/trust-root-flow.md §7.1) —
# nixos-rebuild switch changes the etc entry content, systemd restarts,
# the binary re-reads on startup.
#
# v0.1 surface (tags, healthChecks, metricsPort, dryRun, allowInsecure,
# cacheUrl, healthInterval) was removed in #29 as part of the v0.2
# migration. The v0.2 agent is intentionally minimal; health, metrics,
# and cache concerns move out of the agent binary in this contract.
#
# Auto-included by mkHost (disabled by default).
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;
  nixfleet-agent = inputs.self.packages.${pkgs.system}.nixfleet-agent;

  # Materialise config.nixfleet.trust into the v0.2 proto::TrustConfig
  # JSON shape (crates/nixfleet-proto/src/trust.rs). schemaVersion = 1
  # is required per docs/trust-root-flow.md §7.4 — binaries refuse to
  # start on unknown versions.
  #
  # Shared trust.json payload — see ./_trust-json.nix for shape rationale
  # and the orgRootKey ed25519 promotion that matches proto::TrustConfig.
  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;
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
      default = config.hostSpec.hostName or config.networking.hostName;
      defaultText = lib.literalExpression "config.hostSpec.hostName or config.networking.hostName";
      description = "Machine identifier reported to the control plane.";
    };

    pollInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Poll interval in seconds (steady-state).";
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/agent/trust.json";
      description = ''
        Path to the trust-root JSON file (see docs/trust-root-flow.md §3.4).
        The default is materialised by this module from config.nixfleet.trust
        via environment.etc; override only when sourcing the file from a
        secrets manager.
      '';
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
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      environment.etc."nixfleet/agent/trust.json".source = trustJson;

      systemd.services.nixfleet-agent = {
        description = "NixFleet Fleet Management Agent";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target" "nix-daemon.service"];
        wants = ["network-online.target"];
        startLimitIntervalSec = 0;

        # Agent shells out to nix (copy, path-info) and switch-to-configuration
        path = [config.nix.package pkgs.systemd];

        environment = {
          # Nix writes its metadata cache (narinfo lookups, eval cache, etc.)
          # to $XDG_CACHE_HOME (default: ~/.cache). Point it at the agent's
          # StateDirectory so the cache persists on impermanent hosts instead
          # of being wiped on every reboot.
          XDG_CACHE_HOME = "/var/lib/nixfleet/.cache";
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
              "--trust-file"
              (lib.escapeShellArg (toString cfg.trustFile))
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
    })

    # Impermanence: persist agent state across reboots. Outer mkIf so
    # environment.persistence isn't referenced on non-impermanent hosts.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet"];
    })
  ];
}
