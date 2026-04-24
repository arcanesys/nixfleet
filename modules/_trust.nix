# modules/trust.nix
#
# nixfleet.trust.* — the four trust roots from docs/CONTRACTS.md §II.
# Public keys are declared here; private keys live elsewhere (HSM, host
# SSH key, offline Yubikey) and never enter this module.
#
# Each root supports a `.previous` slot for the 30-day rotation grace
# window and a shared `rejectBefore` timestamp for compromise response.
{
  config,
  lib,
  ...
}: let
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
      type = keySlotType;
      default = {};
      description = ''
        CI release key (ed25519). Private half in Stream A's HSM/TPM;
        public half declared here. Verified by the control plane on every
        `fleet.resolved` fetch. See docs/CONTRACTS.md §II #1.
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
