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
    ...
  }: let
    harness = import ../../tests/harness {inherit lib pkgs inputs;};
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = harness;
    };
}
