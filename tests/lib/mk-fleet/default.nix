{
  lib,
  impl ? import ../../../lib/mk-fleet.nix {inherit lib;},
}: let
  inherit (impl) mkFleet mergeFleets;
  fixtureArgs = {inherit lib mkFleet mergeFleets;};

  runPositive = path: let
    cfg = import path fixtureArgs;
    expectedPath = lib.replaceStrings [".nix"] [".resolved.json"] (toString path);
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
    result = builtins.tryEval (import path fixtureArgs).resolved;
  in
    if result.success
    then throw "expected eval failure for ${toString path}, got success"
    else "ok";

  listFixtures = dir:
    lib.filter (n: lib.hasSuffix ".nix" n && !(lib.hasPrefix "_" n)) (builtins.attrNames (builtins.readDir dir));

  positives = map (n: runPositive (./fixtures + "/${n}")) (listFixtures ./fixtures);
  negatives = map (n: runNegative (./negative + "/${n}")) (listFixtures ./negative);
in {
  results = positives ++ negatives;
}
