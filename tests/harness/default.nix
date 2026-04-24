# tests/harness/default.nix
#
# Entry point for the microvm.nix-based fleet simulation harness (issue #5).
#
# Returns an attrset of discoverable scenarios. `flake-module.nix` registers
# these under `checks.<system>.fleet-harness-*`. Each scenario is a
# standalone runNixOSTest derivation so a failure in one doesn't mask
# the others (same convention as modules/tests/_vm-fleet-scenarios/).
#
# Scaffold scope: one scenario (`smoke`, N=2 agents). The extension path
# for `fleet-N` (the acceptance target from issue #5) is to import a new
# scenario file here and parameterise the agent count — see scenarios/smoke.nix
# for the pattern.
{
  lib,
  pkgs,
  inputs,
  # `nixfleet-canonicalize` is built by the workspace crane pipeline
  # (see `crane-workspace.nix`) and wired in by `modules/tests/harness.nix`.
  # Default to `null` so this file still evaluates from callers that don't
  # pass it — fixture-dependent attrs will throw on access.
  nixfleet-canonicalize ? null,
  # `nixfleet-verify-artifact` is built by the same crane pipeline. The
  # signed-roundtrip scenario invokes it from inside the agent microVM;
  # the smoke scenario does not need it.
  nixfleet-verify-artifact ? null,
}: let
  harnessLib = import ./lib.nix {inherit lib pkgs inputs;};

  # One shared cert set for every harness scenario. When a new scenario
  # needs a new hostname, append it here and it's available to every
  # scenario without rebuilding the others.
  sharedCerts = harnessLib.mkHarnessCerts {
    hostnames = ["cp" "agent-01" "agent-02"];
  };

  scenarioArgs = {
    inherit lib pkgs inputs harnessLib;
    testCerts = sharedCerts;
    resolvedJsonPath = ./fixtures/fleet-resolved.json;
  };

  # Phase 2 PR(a): signed-fixture derivation. Consumed by the
  # `signed-roundtrip` scenario and by `crates/nixfleet-verify-artifact`.
  # See ./fixtures/signed/README.md.
  signedFixture =
    if nixfleet-canonicalize == null
    then
      throw ''
        tests/harness: signedFixture requires `nixfleet-canonicalize` to be
        passed in. Wire it via `modules/tests/harness.nix` or call sites
        that have the flake's `packages.<system>.nixfleet-canonicalize`.
      ''
    else
      import ./fixtures/signed {
        inherit lib pkgs nixfleet-canonicalize;
      };

  # Phase 2 PR(b): signed-roundtrip scenario. Depends on both
  # `signedFixture` (fixture bytes + trust.json) and
  # `nixfleet-verify-artifact` (the CLI the agent microVM runs).
  signedRoundtripScenario =
    if nixfleet-verify-artifact == null
    then
      throw ''
        tests/harness: fleet-harness-signed-roundtrip requires
        `nixfleet-verify-artifact` to be passed in. Wire it via
        `modules/tests/harness.nix` using the crane-built package.
      ''
    else
      import ./scenarios/signed-roundtrip.nix (scenarioArgs
        // {
          inherit signedFixture;
          verifyArtifactPkg = nixfleet-verify-artifact;
        });
in {
  # Target shape per issue #5: `checks.<system>.fleet-N`. For the scaffold
  # we only ship N=2 (smoke). Extension: import additional scenario files
  # here with different agent counts, or parameterise smoke.nix to accept
  # `agentCount` and expose fleet-5, fleet-10 wrappers.
  fleet-harness-smoke = import ./scenarios/smoke.nix scenarioArgs;

  fleet-harness-signed-roundtrip = signedRoundtripScenario;

  # Signed-fixture derivation exposed as a harness attribute. Registered
  # as a flake check (`phase-2-signed-fixture`) in `modules/tests/harness.nix`
  # so byte-stability regressions surface on every CI run.
  inherit signedFixture;
}
