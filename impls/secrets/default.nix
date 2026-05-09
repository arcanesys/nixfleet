# Backend-agnostic secrets wiring: identity paths, host-key bootstrap, persistence contribution.
{
  config,
  lib,
  pkgs,
  ...
}: let
  hS = config.hostSpec;
  cfg = config.nixfleet.secrets;
  types = lib.types;
in {
  options.nixfleet.secrets = {
    enable = lib.mkEnableOption "NixFleet secrets wiring (identity paths, persist, boot ordering)";

    identityPaths = {
      hostKey = lib.mkOption {
        type = types.nullOr types.str;
        default = "/etc/ssh/ssh_host_ed25519_key";
        description = "Primary decryption identity (host SSH key). Used on all hosts.";
      };

      userKey = lib.mkOption {
        type = types.nullOr types.str;
        default = "${hS.home}/.ssh/id_ed25519";
        defaultText = lib.literalExpression ''"''${config.hostSpec.home}/.ssh/id_ed25519"'';
        description = "Fallback decryption identity (user key). Used on workstations only.";
      };

      enableUserKey = lib.mkOption {
        type = types.bool;
        default = true;
        description = "Enable user key as fallback. Roles/profiles that run headless should set this to false.";
      };

      extra = lib.mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Additional identity paths appended to the resolved list.";
      };
    };

    resolvedIdentityPaths = lib.mkOption {
      type = types.listOf types.str;
      readOnly = true;
      internal = true;
      description = ''
        Computed identity paths list. Consumed by fleet secret
        modules (agenix/sops/...) that need the resolved list -
        this is an introspection hook, not an operator option.
      '';
    };
  };

  config = lib.mkMerge [
    # LOADBEARING: resolvedIdentityPaths computed unconditionally so consumers can introspect even when disabled.
    {
      nixfleet.secrets.resolvedIdentityPaths =
        lib.optional (cfg.identityPaths.hostKey != null) cfg.identityPaths.hostKey
        ++ lib.optional (cfg.identityPaths.enableUserKey && cfg.identityPaths.userKey != null) cfg.identityPaths.userKey
        ++ cfg.identityPaths.extra;
    }

    (lib.mkIf cfg.enable {
      # LOADBEARING: generate host SSH key before sshd so secret decryption can use it.
      systemd.services."nixfleet-host-key-check" = lib.mkIf (cfg.identityPaths.hostKey != null) {
        description = "Ensure SSH host key exists for secret decryption";
        wantedBy = ["multi-user.target"];
        before = ["sshd.service"];
        unitConfig.ConditionPathExists = "!${cfg.identityPaths.hostKey}";
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
        };
        script = ''
          ${pkgs.openssh}/bin/ssh-keygen -t ed25519 -f "${cfg.identityPaths.hostKey}" -N ""
        '';
      };

      system.activationScripts.nixfleet-secrets-check = lib.stringAfter ["users"] ''
        for key in ${lib.concatStringsSep " " (map lib.escapeShellArg cfg.resolvedIdentityPaths)}; do
          if [[ ! -f "$key" ]]; then
            echo "WARNING: nixfleet.secrets identity key missing: $key (expected on first boot)"
          fi
        done
      '';
    })

    (lib.mkIf cfg.enable {
      nixfleet.persistence.files =
        lib.optional (cfg.identityPaths.hostKey != null) cfg.identityPaths.hostKey
        ++ lib.optional (cfg.identityPaths.hostKey != null) "${cfg.identityPaths.hostKey}.pub";
    })
  ];
}
