# Secrets wiring — backend-agnostic identity path management.
# Provides identity path computation, impermanence persistence,
# boot ordering (host key generation), and key validation.
# Fleet repos import their chosen backend (agenix, sops-nix) and
# use config.nixfleet.secrets.resolvedIdentityPaths.
# Returns { nixos } module attrset.
# mkHost imports this directly; it activates via nixfleet.secrets.enable.
{
  nixos = {
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
          default = "${hS.home}/.keys/id_ed25519";
          description = "Fallback decryption identity (user key). Used on workstations only.";
        };

        enableUserKey = lib.mkOption {
          type = types.bool;
          default = !hS.isServer;
          description = "Enable user key as fallback. Defaults to true on non-server hosts.";
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
        description = "Computed identity paths list. Fleet modules pass this to agenix/sops.";
      };
    };

    config = lib.mkMerge [
      # Always compute resolvedIdentityPaths (even when not enabled, for introspection)
      {
        nixfleet.secrets.resolvedIdentityPaths =
          lib.optional (cfg.identityPaths.hostKey != null) cfg.identityPaths.hostKey
          ++ lib.optional (cfg.identityPaths.enableUserKey && cfg.identityPaths.userKey != null) cfg.identityPaths.userKey
          ++ cfg.identityPaths.extra;
      }

      # Active config only when enabled
      (lib.mkIf cfg.enable {
        # Impermanence: persist host key only.
        # User key directory (.keys) is persisted by _impermanence.nix (HM level).
        environment.persistence."/persist" = lib.mkIf (hS.isImpermanent or false) {
          files =
            lib.optional (cfg.identityPaths.hostKey != null) cfg.identityPaths.hostKey
            ++ lib.optional (cfg.identityPaths.hostKey != null) "${cfg.identityPaths.hostKey}.pub";
        };

        # Boot ordering: ensure host key exists before sshd
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

        # Key validation: non-fatal warning at activation
        system.activationScripts.nixfleet-secrets-check = lib.stringAfter ["users"] ''
          for key in ${lib.concatStringsSep " " (map lib.escapeShellArg cfg.resolvedIdentityPaths)}; do
            if [[ ! -f "$key" ]]; then
              echo "WARNING: nixfleet.secrets identity key missing: $key (expected on first boot)"
            fi
          done
        '';
      })
    ];
  };
}
