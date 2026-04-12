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
