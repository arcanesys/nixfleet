# NixOS module to configure a host as an Attic binary cache client.
# Adds the cache to nix substituters and installs the attic CLI.
# Auto-included by mkHost (disabled by default).
{
  config,
  lib,
  pkgs,
  inputs,
  ...
}: let
  cfg = config.services.nixfleet-attic-client;
  types = lib.types;
in {
  options.services.nixfleet-attic-client = {
    enable = lib.mkEnableOption "NixFleet Attic binary cache client";

    cacheUrl = lib.mkOption {
      type = types.str;
      example = "https://cache.fleet.example.com";
      description = "URL of the Attic cache server.";
    };

    publicKey = lib.mkOption {
      type = types.str;
      example = "cache.fleet.example.com:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
      description = "Cache signing public key (name:base64).";
    };
  };

  config = lib.mkIf cfg.enable {
    nix.settings = {
      substituters = [cfg.cacheUrl];
      trusted-public-keys = [cfg.publicKey];
    };

    environment.systemPackages = [
      inputs.attic.packages.${pkgs.system}.attic-client
    ];
  };
}
