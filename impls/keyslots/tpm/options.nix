{lib, ...}: let
  keyType = lib.types.submodule ({name, ...}: {
    options = {
      handle = lib.mkOption {
        type = lib.types.str;
        description = ''
          TPM2 persistent handle for this keyslot (TPM2_HT_PERSISTENT range,
          0x81000000-0x817FFFFF). Each named keyslot must have a unique handle.
        '';
      };

      algorithm = lib.mkOption {
        type = lib.types.enum ["ecdsa-p256" "ed25519"];
        default = "ecdsa-p256";
        description = ''
          Signing algorithm. Commodity TPM2 hardware (Intel PTT, AMD fTPM,
          most discrete TPMs) supports RSA + ECDSA P-256 but not ed25519
          (TPM ECC curve 0x0040 is rare). Use ecdsa-p256 unless the TPM
          advertises ed25519 (check with `tpm2_getcap ecc-curves`).
        '';
      };

      pcrPolicy = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = ["0"];
        example = ["0" "2" "4" "7"];
        description = ''
          SHA-256 PCR indices to bind the keyslot's auth policy to. Empty
          list = `userwithauth` (any process with TPM device access can sign;
          no boot-state binding). Default `["0"]` binds to UEFI firmware
          measurement - stable across kernel/bootloader updates, only
          changes on firmware update. See abstracts33d/nixfleet#83 for
          broader policy expansion (PCRs 0/2/4/7) once Secure Boot
          (#84) lands.
        '';
      };

      signWrapperName = lib.mkOption {
        type = lib.types.str;
        default = "tpm-sign-${name}";
        description = ''
          Name of the shell wrapper installed system-wide that signs a
          file with this keyslot's TPM-held key.
        '';
      };
    };
  });
in {
  options.nixfleet.keyslots.tpm = {
    enable = lib.mkEnableOption "TPM2-backed signing keyslot provisioned at first boot";

    exportPubkeyDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-tpm-keyslot";
      description = ''
        Parent directory where exported public key files are written. Per-key
        subdirectories live at `${"$"}{exportPubkeyDir}/<keyname>/pubkey.{pem,raw}`.
        The legacy singleton (set via top-level `handle` / `algorithm`) writes
        to `${"$"}{exportPubkeyDir}/pubkey.{pem,raw}` directly for backward
        compatibility.
      '';
    };

    signWrappers = lib.mkOption {
      type = lib.types.attrsOf lib.types.package;
      readOnly = true;
      description = ''
        Read-only: per-key sign-wrapper derivations, indexed by key name.
        Exposed so consumers can reference the derivation directly
        (e.g. to extend a CI runner's `systemd.services.<name>.path`)
        rather than going through `/run/current-system/sw/bin/<name>`.
      '';
    };

    pubkeyDirs = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      readOnly = true;
      description = ''
        Read-only: per-key directories where exported public key files
        (`pubkey.pem`, `pubkey.raw`) are written, indexed by key name.
      '';
    };

    keys = lib.mkOption {
      type = lib.types.attrsOf keyType;
      default = {};
      example = lib.literalExpression ''
        {
          ciReleaseKey = {
            handle = "0x81010001";
            algorithm = "ecdsa-p256";
            pcrPolicy = ["0"];
          };
          issuanceCA = {
            handle = "0x81010002";
            algorithm = "ecdsa-p256";
            pcrPolicy = ["0"];
          };
        }
      '';
      description = ''
        Named TPM2 persistent keyslots managed by this scope. Each entry
        provisions a primary key at first boot, evicts it to its persistent
        handle, exports the public half, and installs a per-key sign wrapper.
        Multiple named keys can coexist on the same TPM at distinct handles.
      '';
    };

    # ─── Legacy singleton API (predates `keys` attrset) ────────────────────
    # Backward compat: setting any of `handle`, `algorithm`, `signWrapperName`
    # configures an implicit `keys.legacy` entry. New consumers should use
    # `keys.<name>` directly.

    handle = lib.mkOption {
      type = lib.types.str;
      default = "0x81010001";
      description = ''
        Legacy: TPM2 persistent handle. The default of `0x81010001`
        provisions a keyslot named `legacy` automatically when the
        scope is enabled - preserves pre-multi-keyslot behaviour. New
        consumers should use `keys.<name>.handle` instead and avoid
        re-using `0x81010001` (which the legacy keyslot occupies).
      '';
    };

    algorithm = lib.mkOption {
      type = lib.types.enum ["ecdsa-p256" "ed25519"];
      default = "ecdsa-p256";
      description = "Legacy: signing algorithm for the `legacy` keyslot.";
    };

    signWrapperName = lib.mkOption {
      type = lib.types.str;
      default = "tpm-sign";
      description = "Legacy: sign wrapper name for the `legacy` keyslot.";
    };

    signWrapperPackage = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      description = "Legacy: sign wrapper derivation for the `legacy` keyslot.";
    };
  };
}
