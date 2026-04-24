# modules/_trust.nix
#
# nixfleet.trust.* — the four trust roots from docs/CONTRACTS.md §II.
# Public keys are declared here; private keys live elsewhere (HSM, host
# SSH key, offline Yubikey) and never enter this module.
#
# Each root supports a `.previous` slot for the 30-day rotation grace
# window and a shared `rejectBefore` timestamp for compromise response.
#
# `ciReleaseKey` carries an explicit `algorithm` per the §II #1 amendment
# in abstracts33d/nixfleet#18 (ECDSA P-256 alongside ed25519 because
# commodity TPM2 hardware does not expose the ed25519 curve). The other
# trust roots remain bare strings for now; their algorithms are pinned
# by CONTRACTS.md §II #2 (attic-native) and §II #3 (ed25519).
{
  config,
  lib,
  ...
}: let
  # Typed public-key declaration (CONTRACTS §II #1 amendment).
  # Consumers read `.algorithm` to pick the verifier; `.public` is the
  # base64-encoded raw public key bytes per the algorithm's encoding rule.
  publicKeyType = lib.types.submodule {
    options = {
      algorithm = lib.mkOption {
        type = lib.types.enum ["ed25519" "ecdsa-p256"];
        description = ''
          Signing algorithm. `ed25519` is the preferred default for
          HSMs, YubiKeys, cloud KMS, and software-held keys. `ecdsa-p256`
          exists because commodity TPM2 hardware (Intel PTT, AMD fTPM)
          does not expose the ed25519 curve (TPM2_ECC_CURVE_ED25519 =
          0x0040 is rarely implemented). Both produce 64-byte signatures.
        '';
      };
      public = lib.mkOption {
        type = lib.types.str;
        description = ''
          Base64-encoded raw public key bytes.
          - `ed25519` — 32-byte raw pubkey.
          - `ecdsa-p256` — uncompressed point, 64 bytes (`X ‖ Y`, no
            `0x04` prefix). Consumers convert to SEC1 / DER SPKI at
            verify time.
        '';
      };
    };
  };

  # CI-release-key slot. Per CONTRACTS §II #1, the public half is typed
  # (algorithm + public) so consumers learn the algorithm without
  # out-of-band knowledge. `rejectBefore` remains a flat timestamp.
  ciReleaseKeySlotType = lib.types.submodule {
    options = {
      current = lib.mkOption {
        type = lib.types.nullOr publicKeyType;
        default = null;
        description = ''
          Current CI release public key. Set to `null` means no key is
          pinned yet — artifact verification will refuse all signatures.
        '';
      };
      previous = lib.mkOption {
        type = lib.types.nullOr publicKeyType;
        default = null;
        description = ''
          Previous CI release public key accepted during the 30-day
          rotation grace window (CONTRACTS §II #1 rotation procedure).
          May differ in algorithm from `current` — consumers accept
          signatures under either algorithm during the overlap. Remove
          when the window closes.
        '';
      };
      rejectBefore = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          RFC 3339 timestamp. Signed artifacts older than this are refused
          regardless of key. Used in compromise response when rolling out
          a new key is not sufficient (pre-compromise artifacts still
          carry the old key's trust).
        '';
      };
    };
  };

  # Legacy slot type — bare-string current/previous, used by the other
  # two trust roots that still pin a single algorithm per CONTRACTS §II.
  keySlotType = lib.types.submodule {
    options = {
      current = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Current public key (OpenSSH-armored or framework-specific format).";
      };
      previous = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Previous public key accepted during rotation grace. Remove after
          the rotation window closes (see docs/CONTRACTS.md §II for
          per-key grace windows).
        '';
      };
      rejectBefore = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          RFC 3339 timestamp. Signed artifacts older than this are refused
          regardless of key. Used in compromise response when rolling out
          a new key is not sufficient (pre-compromise artifacts still
          carry the old key's trust).
        '';
      };
    };
  };
in {
  options.nixfleet.trust = {
    ciReleaseKey = lib.mkOption {
      type = ciReleaseKeySlotType;
      default = {};
      description = ''
        CI release key. Private half in Stream A's HSM/TPM; public half
        declared here as a typed submodule (`algorithm` + `public`) per
        CONTRACTS.md §II #1. Verified by the control plane on every
        `fleet.resolved` fetch, matching `meta.signatureAlgorithm`.
      '';
    };

    atticCacheKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        Attic binary-cache key. Agents verify before every closure
        activation. See docs/CONTRACTS.md §II #2.
      '';
    };

    orgRootKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        Organization root key. Verifies enrollment tokens at the control
        plane. Rotation is a catastrophic event — see docs/CONTRACTS.md
        §II #3.
      '';
    };
  };

  config.assertions = [
    {
      assertion =
        config.nixfleet.trust.ciReleaseKey.previous
        == null
        || config.nixfleet.trust.ciReleaseKey.current != null;
      message = "nixfleet.trust.ciReleaseKey: cannot set .previous without .current";
    }
    {
      assertion =
        config.nixfleet.trust.atticCacheKey.previous
        == null
        || config.nixfleet.trust.atticCacheKey.current != null;
      message = "nixfleet.trust.atticCacheKey: cannot set .previous without .current";
    }
    {
      assertion =
        config.nixfleet.trust.orgRootKey.previous
        == null
        || config.nixfleet.trust.orgRootKey.current != null;
      message = "nixfleet.trust.orgRootKey: cannot set .previous without .current";
    }
  ];
}
