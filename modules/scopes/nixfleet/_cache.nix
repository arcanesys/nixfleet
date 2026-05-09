{
  config,
  lib,
  ...
}: let
  cfg = config.services.nixfleet-cache;
  types = lib.types;
in {
  options.services.nixfleet-cache = {
    enable = lib.mkEnableOption "NixFleet binary cache client";

    cacheUrl = lib.mkOption {
      type = types.str;
      example = "https://cache.fleet.example.com";
      description = "URL of the binary cache server.";
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
  };
}
