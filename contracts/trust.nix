# LOADBEARING: only public trust roots live here; private keys (HSM, host SSH key, offline Yubikey) never enter this module.
{
  config,
  lib,
  ...
}: let
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
          - `ed25519` - 32-byte raw pubkey.
          - `ecdsa-p256` - uncompressed point, 64 bytes (`X ‖ Y`, no
            `0x04` prefix). Consumers convert to SEC1 / DER SPKI at
            verify time.
        '';
      };
    };
  };

  ciReleaseKeySlotType = lib.types.submodule {
    options = {
      current = lib.mkOption {
        type = lib.types.nullOr publicKeyType;
        default = null;
        description = ''
          Current CI release public key. Set to `null` means no key is
          pinned yet - artifact verification will refuse all signatures.
        '';
      };
      previous = lib.mkOption {
        type = lib.types.nullOr publicKeyType;
        default = null;
        description = ''
          Previous CI release public key accepted during the 30-day
          rotation grace window (CONTRACTS §II #1 rotation procedure).
          May differ in algorithm from `current` - consumers accept
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
      successor = lib.mkOption {
        type = lib.types.nullOr publicKeyType;
        default = null;
        description = ''
          Pre-announced next CI release key (nixfleet#63). Accepted by
          verifiers during the rotation overlap window (now < `retireAt`).
          Past `retireAt`, the reconciler emits
          `Action::RotateTrustRoot`; the operator's tooling rotates
          `current → previous`, `successor → current` in the next
          fleet commit.

          Must be set together with `retireAt` (paired-options
          assertion below). Set both to plan a rotation in advance;
          remove both after the operator commits the rotation.
        '';
      };
      retireAt = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          RFC 3339 deadline for the planned rotation declared in
          `successor`. Drives the verifier's overlap-window check
          AND the reconciler's rotation-due signal.
        '';
      };
    };
  };

  # GOTCHA: bare-string slot for trust roots that pin a single algorithm (vs ciReleaseKey's algo+public submodule).
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
      successor = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Pre-announced next public key (nixfleet#63). Same semantics
          as `ciReleaseKey.successor` - paired with `retireAt`,
          accepted during the overlap window, drives
          `Action::RotateTrustRoot` past the deadline.
        '';
      };
      retireAt = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          RFC 3339 deadline for the planned rotation declared in
          `successor`.
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
        CI release key. Private half in CI's HSM/TPM; public half
        declared here as a typed submodule (`algorithm` + `public`) per
        CONTRACTS.md §II #1. Verified by the control plane on every
        `fleet.resolved` fetch, matching `meta.signatureAlgorithm`.
      '';
    };

    cacheKeys = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      example = [
        "cache.example.com:AAAA..."
        "attic:cache.example.com:BBBB..."
      ];
      description = ''
        Trusted public keys for the binary cache(s) the fleet
        substitutes from. Forwarded opaquely to nix as
        `nix.settings.trusted-public-keys`. Format depends on the
        cache implementation:

        - harmonia / nix-serve / cachix: `<name>:<base64>`
        - attic: `attic:<host>:<base64>`

        Both are accepted by nix; mix freely. Empty list is fine for
        fleets with no shared cache or that distribute trust through
        another channel. See docs/CONTRACTS.md §II #2.
      '';
    };

    orgRootKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        Organization root key. Verifies enrollment tokens at the control
        plane. Rotation is a catastrophic event - see docs/CONTRACTS.md
        §II #3.
      '';
    };

    rootCAPem = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        PEM-encoded fleet root CA certificate. Offline-signed on the
        operator workstation (file or Yubikey per D12); embedded in
        trust.json so verifiers can anchor cert chains at a key the CP
        never holds at rest. `null` until the operator has run
        `nixfleet-trust-bootstrap`.
      '';
    };

    issuanceCAPems = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = ''
        PEM-encoded issuance CA chain. Each entry is signed by
        `rootCAPem` and represents an issuance CA the fleet currently
        trusts to mint agent certs. Multiple entries during a rotation
        overlap window - agents accept any cert chain anchored at one
        of these intermediates. The TPM-bound issuance CA on the CP
        host appears here once it's bootstrapped.
      '';
    };
  };

  config.assertions = let
    pairedSuccessor = slot: name: [
      {
        assertion =
          (slot.successor == null) == (slot.retireAt == null);
        message = "nixfleet.trust.${name}: `successor` and `retireAt` are paired - set both or neither (nixfleet#63 rotation declarative-pre-announcement)";
      }
    ];
  in
    [
      {
        assertion =
          config.nixfleet.trust.ciReleaseKey.previous
          == null
          || config.nixfleet.trust.ciReleaseKey.current != null;
        message = "nixfleet.trust.ciReleaseKey: cannot set .previous without .current";
      }
      {
        assertion =
          config.nixfleet.trust.orgRootKey.previous
          == null
          || config.nixfleet.trust.orgRootKey.current != null;
        message = "nixfleet.trust.orgRootKey: cannot set .previous without .current";
      }
    ]
    ++ (pairedSuccessor config.nixfleet.trust.ciReleaseKey "ciReleaseKey")
    ++ (pairedSuccessor config.nixfleet.trust.orgRootKey "orgRootKey");
}
