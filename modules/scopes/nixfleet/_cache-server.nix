# NixOS service module for the NixFleet binary cache server (harmonia).
# Thin wrapper around the upstream services.harmonia NixOS module.
# Serves paths directly from the local Nix store over HTTP.
# Auto-included by mkHost (disabled by default).
{
  config,
  lib,
  ...
}: let
  cfg = config.services.nixfleet-cache-server;
in {
  options.services.nixfleet-cache-server = {
    enable = lib.mkEnableOption "NixFleet binary cache server (harmonia)";

    port = lib.mkOption {
      type = lib.types.port;
      default = 5000;
      description = "Port to listen on.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the cache server port in the firewall.";
    };

    signingKeyFile = lib.mkOption {
      type = lib.types.str;
      example = "/run/secrets/cache-signing-key";
      description = "Path to the Nix signing key file for on-the-fly signing.";
    };
  };

  config = lib.mkIf cfg.enable {
    # Delegate to upstream harmonia NixOS module
    services.harmonia.cache = {
      enable = true;
      signKeyPaths = [cfg.signingKeyFile];
      settings.bind = "0.0.0.0:${toString cfg.port}";
    };

    # Sign paths at build/copy time (needed for nix copy --to ssh://host)
    nix.settings.secret-key-files = [cfg.signingKeyFile];

    # Open firewall port if requested
    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [cfg.port];
  };
}
