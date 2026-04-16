# Specifications For Differentiating Hosts
#
# Framework-level options only. Fleet-specific options (isDev, isGraphical,
# useHyprland, theme, githubUser, etc.) are declared in the consuming fleet
# repo via plain NixOS modules that extend hostSpec.
{
  config,
  lib,
  ...
}: {
  options.hostSpec = {
    # Data variables that don't dictate configuration settings
    userName = lib.mkOption {
      type = lib.types.str;
      description = "The username of the host";
    };
    hostName = lib.mkOption {
      type = lib.types.str;
      description = "The hostname of the host";
    };
    networking = lib.mkOption {
      default = {};
      type = lib.types.attrsOf lib.types.anything;
      description = "An attribute set of networking information";
    };

    secretsPath = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Hint for secrets repo path. Framework-agnostic — no agenix coupling.";
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
    sshAuthorizedKeys = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "SSH public keys for authorized_keys (primary user and root).";
    };

    home = lib.mkOption {
      type = lib.types.str;
      description = "The home directory of the user";
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

    # Configuration Settings
    isMinimal = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Used to indicate a minimal host";
    };
    isDarwin = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Used to indicate a host that is darwin";
    };
    isImpermanent = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Used to indicate an impermanent host";
    };
    isServer = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Used to indicate a server host";
    };
    managedUser = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        When true (default), NixFleet core creates the primary user at
        users.users.''${hostSpec.userName}. Set to false on hosts where
        a different module owns the user set (e.g. Sécurix endpoints with
        an operator inventory). Does not affect root.
      '';
    };
    enableHomeManager = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        When true (default), NixFleet injects Home Manager as a NixOS
        module and configures home-manager.users.''${hostSpec.userName}.
        Set to false on hosts that don't use per-user HM config (e.g.
        locked-down endpoints with multi-operator login).
      '';
    };
    customFilesystems = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        When true, NixFleet skips its built-in disk/filesystem imports
        (qemu disk config for VMs; any future default disko template).
        Set this on hosts that provide their own disko layout, e.g.
        Sécurix endpoints with securix.filesystems.layout = "securix_v1".
      '';
    };
    skipDefaultFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        When true, NixFleet's firewall scope (nftables + SSH rate-limit)
        does not activate. Set this on hosts whose consuming modules own
        the firewall (e.g. Sécurix endpoints with a strict VPN firewall).
      '';
    };
    hashedPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Path to hashed password file for primary user. Null = no managed password.";
    };
    rootHashedPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Path to hashed password file for root. Null = no managed password.";
    };
  };
}
