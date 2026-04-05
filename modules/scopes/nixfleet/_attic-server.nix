# NixOS service module for the NixFleet Attic binary cache server.
# Wraps atticd with opinionated defaults for fleet use.
# Auto-included by mkHost (disabled by default).
{
  config,
  lib,
  pkgs,
  inputs,
  ...
}: let
  cfg = config.services.nixfleet-attic-server;
  types = lib.types;

  storageConfig =
    if cfg.storage.type == "local"
    then {
      type = "local";
      path = cfg.storage.local.path;
    }
    else {
      type = "s3";
      bucket = cfg.storage.s3.bucket;
      region = cfg.storage.s3.region;
      endpoint =
        lib.optionalAttrs (cfg.storage.s3.endpoint != null)
        {inherit (cfg.storage.s3) endpoint;};
    };

  serverToml = pkgs.writeText "attic-server.toml" ''
    listen = "${cfg.listen}"

    [database]
    url = "sqlite://${cfg.dbPath}?mode=rwc"

    [storage]
    type = "${storageConfig.type}"
    ${
      if storageConfig.type == "local"
      then ''path = "${storageConfig.path}"''
      else ''
        bucket = "${storageConfig.bucket}"
        region = "${storageConfig.region}"
        ${lib.optionalString (cfg.storage.s3.endpoint != null) ''endpoint = "${cfg.storage.s3.endpoint}"''}
      ''
    }

    [garbage-collection]
    default-retention-period = "${cfg.garbageCollection.keepSinceLastPush}"

    # TODO(#22): revert when upstream Attic merges PR #300 — move token back to CLI flag
    # booxter/newer-nix removed --token-hs256-secret-base64 CLI arg, expects it in [jwt.signing]
    [jwt.signing]
    token-hs256-secret-base64 = "dW51c2VkLXBsYWNlaG9sZGVyLWZvci1hdHRpYy1zZXJ2ZXI="

    [chunking]
    nar-size-threshold = 65536
    min-size = 16384
    avg-size = 65536
    max-size = 262144
  '';
in {
  options.services.nixfleet-attic-server = {
    enable = lib.mkEnableOption "NixFleet Attic binary cache server";

    listen = lib.mkOption {
      type = types.str;
      default = "0.0.0.0:8081";
      description = "Address and port to listen on.";
    };

    openFirewall = lib.mkOption {
      type = types.bool;
      default = false;
      description = "Open the Attic server port in the firewall.";
    };

    dbPath = lib.mkOption {
      type = types.str;
      default = "/var/lib/nixfleet-attic/server.db";
      description = "Path to the SQLite database.";
    };

    signingKeyFile = lib.mkOption {
      type = types.str;
      example = "/run/secrets/attic-signing-key";
      description = "Path to the cache signing key file.";
    };

    storage = {
      type = lib.mkOption {
        type = types.enum ["local" "s3"];
        default = "local";
        description = "Storage backend type.";
      };

      local.path = lib.mkOption {
        type = types.str;
        default = "/var/lib/nixfleet-attic/storage";
        description = "Local filesystem storage path.";
      };

      s3 = {
        bucket = lib.mkOption {
          type = types.str;
          default = "";
          description = "S3 bucket name.";
        };

        region = lib.mkOption {
          type = types.str;
          default = "";
          description = "S3 region.";
        };

        endpoint = lib.mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "S3-compatible endpoint URL (e.g. MinIO).";
        };
      };
    };

    garbageCollection = {
      schedule = lib.mkOption {
        type = types.str;
        default = "weekly";
        description = "Systemd calendar expression for garbage collection.";
      };

      keepSinceLastPush = lib.mkOption {
        type = types.str;
        default = "90 days";
        description = "Duration to keep paths after last push.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.nixfleet-attic-server = {
      description = "NixFleet Attic Binary Cache Server";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      serviceConfig = {
        Type = "simple";
        ExecStart = "${inputs.attic.packages.${pkgs.system}.attic-server}/bin/atticd --config ${serverToml}";
        Restart = "always";
        RestartSec = 10;
        StateDirectory = "nixfleet-attic";

        # Hardening
        NoNewPrivileges = true;
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ReadWritePaths = ["/var/lib/nixfleet-attic"];
      };
    };

    # Garbage collection timer
    systemd.timers.nixfleet-attic-gc = {
      wantedBy = ["timers.target"];
      timerConfig = {
        OnCalendar = cfg.garbageCollection.schedule;
        Persistent = true;
        RandomizedDelaySec = "1h";
      };
    };

    systemd.services.nixfleet-attic-gc = {
      description = "NixFleet Attic Garbage Collection";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${inputs.attic.packages.${pkgs.system}.attic-server}/bin/atticd --config ${serverToml} --mode garbage-collector-once";
        ReadWritePaths = ["/var/lib/nixfleet-attic"];
      };
    };

    # Open firewall port if requested
    networking.firewall.allowedTCPPorts = let
      port = lib.toInt (lib.last (lib.splitString ":" cfg.listen));
    in
      lib.mkIf cfg.openFirewall [port];

    # Impermanence: persist Attic state
    environment.persistence."/persist".directories =
      lib.mkIf
      (config.hostSpec.isImpermanent or false)
      ["/var/lib/nixfleet-attic"];
  };
}
