# NixOS service module for the NixFleet control plane server (v0.2).
#
# Reads a trust-root declaration from /etc/nixfleet/cp/trust.json and
# consumes a fleet.resolved.json artifact at a local path (git-pull
# pattern, trust-root-flow.md §4 option b). Reload model is restart-only
# (docs/trust-root-flow.md §7.1).
#
# Auto-included by mkHost (disabled by default).
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-control-plane;
  nixfleet-control-plane = inputs.self.packages.${pkgs.system}.nixfleet-control-plane;

  # Shared trust.json payload — see ./_trust-json.nix for shape rationale
  # and the orgRootKey ed25519 promotion that matches proto::TrustConfig.
  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;
in {
  options.services.nixfleet-control-plane = {
    enable = lib.mkEnableOption "NixFleet control plane server";

    listen = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0:8080";
      description = "Address and port to listen on.";
    };

    dbPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/state.db";
      description = "Path to the SQLite state database.";
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/cp/trust.json";
      description = ''
        Path to the trust-root JSON file (see docs/trust-root-flow.md §3.4).
        The default is materialised by this module from config.nixfleet.trust
        via environment.etc.
      '';
    };

    releasePath = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/nixfleet-cp/fleet.git/releases/fleet.resolved.json";
      description = ''
        Path to the resolved fleet artifact (fleet.resolved.json). v0.2
        uses the local-checkout distribution pattern described in
        docs/trust-root-flow.md §4 option (b): a separate systemd timer
        keeps /var/lib/nixfleet-cp/fleet.git/ in sync with the fleet repo,
        and the control plane reads the signed artifact from under it.
      '';
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the control plane port in the firewall.";
    };

    tls = {
      cert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-cert.pem";
        description = "Path to TLS certificate PEM file. Enables HTTPS when set (requires tls.key).";
      };

      key = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-key.pem";
        description = "Path to TLS private key PEM file.";
      };

      clientCa = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/fleet-ca.pem";
        description = "Path to client CA PEM file. Enables mTLS agent authentication when set.";
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = builtins.match ".*:[0-9]+" cfg.listen != null;
          message = ''
            services.nixfleet-control-plane.listen must be in HOST:PORT format
            (e.g. "0.0.0.0:8080"), got: "${cfg.listen}"
          '';
        }
      ];

      environment.etc."nixfleet/cp/trust.json".source = trustJson;

      systemd.services.nixfleet-control-plane = {
        description = "NixFleet Control Plane Server";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];

        serviceConfig = {
          Type = "simple";
          ExecStart = lib.concatStringsSep " " (
            [
              "${nixfleet-control-plane}/bin/nixfleet-control-plane"
              "--listen"
              (lib.escapeShellArg cfg.listen)
              "--db-path"
              (lib.escapeShellArg cfg.dbPath)
              "--trust-file"
              (lib.escapeShellArg (toString cfg.trustFile))
              "--release-path"
              (lib.escapeShellArg (toString cfg.releasePath))
            ]
            ++ lib.optionals (cfg.tls.cert != null) [
              "--tls-cert"
              (lib.escapeShellArg cfg.tls.cert)
            ]
            ++ lib.optionals (cfg.tls.key != null) [
              "--tls-key"
              (lib.escapeShellArg cfg.tls.key)
            ]
            ++ lib.optionals (cfg.tls.clientCa != null) [
              "--client-ca"
              (lib.escapeShellArg cfg.tls.clientCa)
            ]
          );
          Restart = "always";
          RestartSec = 10;
          StateDirectory = "nixfleet-cp";

          # Hardening
          NoNewPrivileges = true;
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          ReadWritePaths = ["/var/lib/nixfleet-cp"];
        };
      };

      # Open firewall port if requested
      networking.firewall.allowedTCPPorts = let
        port = lib.toInt (lib.last (lib.splitString ":" cfg.listen));
      in
        lib.mkIf cfg.openFirewall [port];
    })

    # Impermanence: persist CP state across reboots. Outer mkIf so
    # environment.persistence isn't referenced on non-impermanent hosts.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet-cp"];
    })
  ];
}
