# hostSpec - identity carrier for every host.
#
# Framework-level options only - scope/role/profile/hardware concerns
# live elsewhere:
# - `nixfleet.<scope>.*` options come from `arcanesys/nixfleet-scopes`
# - `fleet.*` options come from the consuming fleet
#
# Posture flags (`isImpermanent`, `isServer`, `isMinimal`) that were
# here in earlier revisions of NixFleet have been removed - their roles
# are now played by scope `enable` options (set by roles) in
# nixfleet-scopes.
{
  config,
  lib,
  ...
}: {
  options.hostSpec = {
    # --- Identity ---
    hostName = lib.mkOption {
      type = lib.types.str;
      description = "The hostname of the host";
    };
    userName = lib.mkOption {
      type = lib.types.str;
      default =
        if config ? nixfleet.operators._primaryName
        then config.nixfleet.operators._primaryName
        else throw "hostSpec.userName: set explicitly or define nixfleet.operators";
      description = "Primary user name. Auto-derived from nixfleet.operators.primaryUser when operators scope is active.";
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

    # --- Locale / keyboard ---
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

    # --- Access ---
    rootHashedPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Path to hashed password file for root. Null = no managed password.";
    };

    # --- Networking ---
    networking = lib.mkOption {
      default = {};
      type = lib.types.attrsOf lib.types.anything;
      description = "An attribute set of networking information (e.g. `interface` hint for DHCP).";
    };

    # --- Secrets backend hint (backend-agnostic) ---
    secretsPath = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Hint for secrets repo path. Framework-agnostic - no agenix coupling.";
    };

    # --- Platform ---
    isDarwin = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether this host runs nix-darwin. Set automatically by mkHost for Darwin platforms.";
    };
  };
}
