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
  happy =
    (lib.evalModules {
      modules = [
        ../_trust.nix
        {
          options.assertions = lib.mkOption {
            type = lib.types.listOf lib.types.unspecified;
            default = [];
          };
        }
        {
          nixfleet.trust = {
            ciReleaseKey.current = {
              algorithm = "ecdsa-p256";
              public = "AAAAcdsap256placeholderbase64bytesXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
            };
            atticCacheKey.current = "attic:cache.example.com:AAAA...";
          };
        }
      ];
      specialArgs = {inherit pkgs;};
    })
    .config;
in {
  happyCiKeyAlgorithm = happy.nixfleet.trust.ciReleaseKey.current.algorithm;
  happyCiKeyPublicNonEmpty = (builtins.stringLength happy.nixfleet.trust.ciReleaseKey.current.public) > 0;
  happyAtticKey = happy.nixfleet.trust.atticCacheKey.current;
  happyOrgKeyDefaultsToNull = happy.nixfleet.trust.orgRootKey.current;
  assertionsDeclared = builtins.length happy.assertions;
}
