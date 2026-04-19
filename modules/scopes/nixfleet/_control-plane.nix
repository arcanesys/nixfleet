# NixOS service module for the NixFleet control plane server.
# Auto-included by mkHost (disabled by default).
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-control-plane;
  nixfleet-control-plane = pkgs.callPackage ../../../crates/control-plane {inherit inputs;};
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
