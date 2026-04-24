# modules/tests/trust-options.nix
#
# Eval test for modules/trust.nix. Verifies the option tree shape.
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
          nixfleet.trust = {
            ciReleaseKey.current = "ssh-ed25519 AAAA...ci";
            atticCacheKey.current = "attic:cache.example.com:AAAA...";
          };
        }
      ];
      specialArgs = {inherit pkgs;};
    })
    .config;
in {
  happyCiKey = happy.nixfleet.trust.ciReleaseKey.current;
  happyAtticKey = happy.nixfleet.trust.atticCacheKey.current;
  happyOrgKeyDefaultsToNull = happy.nixfleet.trust.orgRootKey.current;
  assertionsDeclared = builtins.length happy.assertions;
}
