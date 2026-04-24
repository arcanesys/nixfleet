# modules/tests/_trust-options.nix
#
# Eval test for modules/_trust.nix. Verifies the option tree shape,
# including the typed ciReleaseKey submodule per CONTRACTS §II #1.
#
# Declares `assertions` as a freeform option so the _trust.nix
# assertions (which target the full NixOS module system at host build
# time) can be materialized here in a bare lib.evalModules context
# without a missing-option error.
{
  lib,
  pkgs,
  ...
}: let
  assertionsShim = {
    options.assertions = lib.mkOption {
      type = lib.types.listOf lib.types.unspecified;
      default = [];
    };
  };

  evalTrustWith = ciReleaseKey:
    (lib.evalModules {
      modules = [
        ../_trust.nix
        assertionsShim
        {
          nixfleet.trust = {
            inherit ciReleaseKey;
            atticCacheKey.current = "attic:cache.example.com:AAAA...";
          };
        }
      ];
      specialArgs = {inherit pkgs;};
    })
    .config;

  p256Key = {
    algorithm = "ecdsa-p256";
    public = "AAAAcdsap256placeholderbase64bytesXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
  };
  ed25519Key = {
    algorithm = "ed25519";
    public = "AAAAed25519placeholderbase64bytesXXXXXXXXXXXXXX";
  };

  happyP256 = evalTrustWith {current = p256Key;};
  happyEd25519 = evalTrustWith {current = ed25519Key;};
in {
  # P-256 fixture — covers the TPM-backed path.
  happyP256CiKeyAlgorithm = happyP256.nixfleet.trust.ciReleaseKey.current.algorithm;
  happyP256CiKeyPublicNonEmpty = (builtins.stringLength happyP256.nixfleet.trust.ciReleaseKey.current.public) > 0;

  # ed25519 fixture — covers the HSM / YubiKey / software-key path,
  # asserting the submodule accepts both algorithms symmetrically.
  happyEd25519CiKeyAlgorithm = happyEd25519.nixfleet.trust.ciReleaseKey.current.algorithm;
  happyEd25519CiKeyPublicNonEmpty = (builtins.stringLength happyEd25519.nixfleet.trust.ciReleaseKey.current.public) > 0;

  happyAtticKey = happyP256.nixfleet.trust.atticCacheKey.current;
  happyOrgKeyDefaultsToNull = happyP256.nixfleet.trust.orgRootKey.current;
  assertionsDeclared = builtins.length happyP256.assertions;
}
