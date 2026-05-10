{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    config,
    ...
  }: let
    nixfleet-canonicalize = config.packages.nixfleet-canonicalize or null;
    nixfleet-verify-artifact = config.packages.nixfleet-verify-artifact or null;
    nixfleet-control-plane = config.packages.nixfleet-control-plane or null;
    nixfleet-agent = config.packages.nixfleet-agent or null;
    nixfleet-cli = config.packages.nixfleet-cli or null;
    harness = import ../../tests/harness {
      inherit lib pkgs inputs nixfleet-canonicalize nixfleet-verify-artifact;
      inherit nixfleet-control-plane nixfleet-agent nixfleet-cli;
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks =
        {
          fleet-harness-smoke = harness.fleet-harness-smoke;
        }
        // lib.optionalAttrs (nixfleet-canonicalize != null) {
          signed-fixture = harness.signedFixture;
          revocations-fixture = harness.revocationsFixture;
        }
        // lib.optionalAttrs (nixfleet-canonicalize != null && nixfleet-verify-artifact != null) {
          fleet-harness-signed-roundtrip = harness.fleet-harness-signed-roundtrip;
          fleet-harness-auditor-chain = harness.fleet-harness-auditor-chain;
          fleet-harness-corruption-rejection = harness.fleet-harness-corruption-rejection;
          fleet-harness-future-dated-rejection =
            harness.fleet-harness-future-dated-rejection;
          fleet-harness-manifest-tamper-rejection =
            harness.fleet-harness-manifest-tamper-rejection;
          probe-fixture = harness.probeFixture;
          rollout-manifest-fixture = harness.rolloutManifestFixture;
        }
        // lib.optionalAttrs (
          nixfleet-canonicalize
          != null
          && nixfleet-control-plane != null
          && nixfleet-agent != null
        ) {
          fleet-harness-teardown = harness.fleet-harness-teardown;
          fleet-harness-deadline-expiry = harness.fleet-harness-deadline-expiry;
          fleet-harness-stale-target = harness.fleet-harness-stale-target;
          fleet-harness-boot-recovery = harness.fleet-harness-boot-recovery;
          fleet-harness-fleet-2 = harness.fleet-harness-fleet-2;
          fleet-harness-fleet-5 = harness.fleet-harness-fleet-5;
          fleet-harness-fleet-10 = harness.fleet-harness-fleet-10;
          fleet-harness-secret-hygiene = harness.fleet-harness-secret-hygiene;
          fleet-harness-module-rollouts-wire = harness.fleet-harness-module-rollouts-wire;
          fleet-harness-rollback-policy = harness.fleet-harness-rollback-policy;
          fleet-harness-concurrent-checkin = harness.fleet-harness-concurrent-checkin;
        }
        // lib.optionalAttrs (
          nixfleet-canonicalize
          != null
          && nixfleet-control-plane != null
          && nixfleet-cli != null
        ) {
          fleet-harness-enroll-replay = harness.fleet-harness-enroll-replay;
        };
    };
}
