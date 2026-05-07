# hostSpec — identity carrier for every host.
{
  config,
  lib,
  ...
}: {
  options.hostSpec = {
    hostName = lib.mkOption {
      type = lib.types.str;
      description = "The hostname of the host";
    };
    userName = lib.mkOption {
      type = lib.types.str;
      description = ''
        Primary user name on this host. Set explicitly, or populated by
        a fleet-side operators scope from its own
        `nixfleet.operators.primaryUser` (or equivalent) namespace.
      '';
    };
    home = lib.mkOption {
      type = lib.types.str;
      description = "The home directory of the primary user";
      default = let
        hS = config.hostSpec;
      in
        if hS.isDarwin
        then "/Users/${hS.userName}"
        else "/home/${hS.userName}";
      defaultText = lib.literalExpression ''
        if config.hostSpec.isDarwin
        then "/Users/''${config.hostSpec.userName}"
        else "/home/''${config.hostSpec.userName}"
      '';
    };

    timeZone = lib.mkOption {
      type = lib.types.str;
      default = "UTC";
      description = "IANA timezone (e.g. Europe/Paris)";
    };
    locale = lib.mkOption {
      type = lib.types.str;
      default = "en_US.UTF-8";
      description = "System locale";
    };
    keyboardLayout = lib.mkOption {
      type = lib.types.str;
      default = "us";
      description = "XKB keyboard layout";
    };

    rootHashedPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Path to hashed password file for root. Null = no managed password.";
    };

    rootSshKeys = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = ''
        SSH public keys authorized for root login. Empty list = no
        managed root keys. Populated either by hand or automatically
        by a fleet-side operators scope from its own
        `nixfleet.operators.rootSshKeys` (or equivalent) namespace.
      '';
    };

    networking = lib.mkOption {
      default = {};
      type = lib.types.attrsOf lib.types.anything;
      description = "An attribute set of networking information (e.g. `interface` hint for DHCP).";
    };

    vmPortForwards = lib.mkOption {
      default = {};
      type = lib.types.attrsOf lib.types.port;
      example = lib.literalExpression ''
        {
          "80" = 2280;   # host:2280 -> guest:80
          "443" = 2443;  # host:2443 -> guest:443
        }
      '';
      description = ''
        Per-host host:guest port forwards for `mkVmApps`-driven QEMU runs
        (issue #87). Keys are guest ports (as strings — Nix attribute
        names must be strings); values are host ports. SSH-on-22 always
        forwards via the auto-assigned `SSH_PORT` (or `--ssh-port`
        override) — these are additional ports for guest services the
        operator wants to reach from the host (web servers, control-plane
        TLS, Forgejo, etc.). Empty by default — only consumed when the
        host is run as a VM via `nixfleet build-vm` / `start-vm`.
      '';
    };

    secretsPath = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Hint for secrets repo path. Framework-agnostic - no agenix coupling.";
    };

    isDarwin = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether this host runs nix-darwin. Set automatically by mkHost for Darwin platforms.";
    };
  };
}
