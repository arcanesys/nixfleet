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
    # Pull the canonicalize package from the crane workspace (same
    # perSystem, declared in `modules/rust-packages.nix`). The harness
    # entry point needs it to bake the signed fixture at build time.
    nixfleet-canonicalize = config.packages.nixfleet-canonicalize or null;
    harness = import ../../tests/harness {
      inherit lib pkgs inputs nixfleet-canonicalize;
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
        };
    };
}
