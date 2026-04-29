# LOADBEARING: `assertions` shim makes the contract module evaluable outside a full NixOS module-system context.
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
        ../../contracts/trust.nix
        assertionsShim
        {
          nixfleet.trust = {
            inherit ciReleaseKey;
            cacheKeys = ["attic:cache.example.com:AAAA..."];
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
  happyP256CiKeyAlgorithm = happyP256.nixfleet.trust.ciReleaseKey.current.algorithm;
  happyP256CiKeyPublicNonEmpty = (builtins.stringLength happyP256.nixfleet.trust.ciReleaseKey.current.public) > 0;

  happyEd25519CiKeyAlgorithm = happyEd25519.nixfleet.trust.ciReleaseKey.current.algorithm;
  happyEd25519CiKeyPublicNonEmpty = (builtins.stringLength happyEd25519.nixfleet.trust.ciReleaseKey.current.public) > 0;

  happyCacheKeysCount = builtins.length happyP256.nixfleet.trust.cacheKeys;
  happyOrgKeyDefaultsToNull = happyP256.nixfleet.trust.orgRootKey.current;
  assertionsDeclared = builtins.length happyP256.assertions;
}
