# Tier A - microvm.nix-based fleet simulation harness (issue #5).
#
# Registers `checks.x86_64-linux.fleet-harness-*` discoverable scenarios.
# Each scenario boots one CP microVM + N agent microVMs on a single host
# VM, with /nix/store shared over virtiofs for near-zero cold-start cost.
#
# DIFFERENT from modules/tests/vm-fleet-scenarios.nix: that file wires
# full-closure v0.1 agent/CP nodes through pkgs.testers.runNixOSTest with
# nothing microvm-related. The harness here uses astro/microvm.nix guests.
# Do NOT unify the two substrates - they solve different problems.
#
# Run (once the user is ready):
#   nix build .#checks.x86_64-linux.fleet-harness-smoke --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    config,
    ...
  }: let
    # Pull crane-built packages from the workspace (same perSystem,
    # declared in `modules/rust-packages.nix`). The harness entry point
    # uses `nixfleet-canonicalize` to bake the signed fixture and
    # `nixfleet-verify-artifact` as the binary the signed-roundtrip
    # agent microVM runs.
    nixfleet-canonicalize = config.packages.nixfleet-canonicalize or null;
    nixfleet-verify-artifact = config.packages.nixfleet-verify-artifact or null;
    harness = import ../../tests/harness {
      inherit lib pkgs inputs nixfleet-canonicalize nixfleet-verify-artifact;
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks =
        {
          fleet-harness-smoke = harness.fleet-harness-smoke;
        }
        # Only register the signed-fixture check when the canonicalize
        # package is available for this system (x86_64-linux only today;
        # other systems skip it silently).
        // lib.optionalAttrs (nixfleet-canonicalize != null) {
          # Phase 2 PR(a) signed-fixture derivation. Byte-stability
          # regression guard; rebuild failure signals non-determinism in
          # mkFleet, canonicalize, or the keygen helper.
          phase-2-signed-fixture = harness.signedFixture;
        }
        // lib.optionalAttrs (nixfleet-canonicalize != null && nixfleet-verify-artifact != null) {
          # Phase 2 PR(b) signed-roundtrip scenario. Exercises the full
          # stack: fixture build -> mTLS serve -> agent fetch ->
          # verify_artifact accept -> OK marker.
          fleet-harness-signed-roundtrip = harness.fleet-harness-signed-roundtrip;
        };
    };
}
