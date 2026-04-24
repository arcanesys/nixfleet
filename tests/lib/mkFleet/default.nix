# tests/lib/mkFleet/default.nix
#
# Eval-only tests for lib/mkFleet.nix. No VM, no build — pure evaluation.
# Each .nix file under ./fixtures/ is a positive scenario (must eval clean).
# Each .nix file under ./negative/ is expected to `throw` a specific error.
{
  lib,
  mkFleet ? (import ../../../lib/mkFleet.nix {inherit lib;}).mkFleet,
}: let
  runPositive = path: let
    cfg = import path {inherit lib mkFleet;};
    expectedPath = lib.replaceStrings [".nix"] [".resolved.json"] path;
    expected = builtins.fromJSON (builtins.readFile expectedPath);
    actual = cfg.resolved;
    match = builtins.toJSON actual == builtins.toJSON expected;
  in
    if match
    then "ok"
    else
      throw ''
        golden mismatch for ${toString path}
        expected: ${builtins.toJSON expected}
        actual:   ${builtins.toJSON actual}
      '';

  runNegative = path: let
    result = builtins.tryEval (import path {inherit lib mkFleet;}).resolved;
  in
    if result.success
    then throw "expected eval failure for ${toString path}, got success"
    else "ok";

  listFixtures = dir:
    lib.filter (n: lib.hasSuffix ".nix" n) (builtins.attrNames (builtins.readDir dir));

  positives = map (n: runPositive (./fixtures + "/${n}")) (listFixtures ./fixtures);
  negatives = map (n: runNegative (./negative + "/${n}")) (listFixtures ./negative);
in {
  results = positives ++ negatives;
}
