# tests/harness/scenarios/signed-roundtrip.nix
#
# Phase 2 cross-stream wire-up test per docs/phase-2-entry-spec.md §1–3.
#
# Proves that one signed fleet.resolved.json round-trips through:
#   mkFleet -> withSignature -> nixfleet-canonicalize -> ed25519-sign
#   (fixture build)
#   -> CP mTLS serve -> agent mTLS fetch
#   -> nixfleet-verify-artifact (trust.json + verify_artifact) -> ok marker
#
# Failure in any step surfaces as either `harness-roundtrip-FAIL:` (fetch
# / shape error, surfaced before verify) or a `VerifyError` variant on
# stderr (verify_artifact rejected). Both are visible to the scenario's
# grep on `harness-roundtrip-ok:`, which never appears in those paths.
#
# Sibling-scenario extension path (Checkpoint 2):
#   - copy this file, tamper one byte in `canonical.json` or `.sig`
#     before the CP serves it, assert `harness-roundtrip-ok:` does NOT
#     appear within the deadline -> tamper refusal scenario.
#   - copy, shift agent's `--now` past `signedAt + freshnessWindow`,
#     assert Stale -> freshness refusal scenario.
{
  harnessLib,
  testCerts,
  signedFixture,
  verifyArtifactPkg,
  ...
}: let
  cpHostModule = harnessLib.mkSignedCpHostModule {
    inherit testCerts signedFixture;
  };

  agent = harnessLib.mkVerifyingAgentNode {
    inherit testCerts signedFixture verifyArtifactPkg;
    hostName = "agent-01";
  };
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-signed-roundtrip";
    inherit cpHostModule;
    agents = {agent-01 = agent;};
    timeout = 600;
    testScript = ''
      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("harness-cp.service")
      host.wait_for_open_port(8443)

      host.wait_for_unit("microvms.target", timeout=300)
      host.wait_for_unit("microvm@agent-01.service", timeout=300)

      import time
      deadline = time.monotonic() + 120
      while time.monotonic() < deadline:
          rc, _ = host.execute(
              "journalctl -u microvm@agent-01.service --no-pager "
              "| grep -q 'harness-roundtrip-ok:'"
          )
          if rc == 0:
              break
          time.sleep(2)
      else:
          raise Exception("agent did not emit harness-roundtrip-ok within 120s")

      rc, _ = host.execute(
          "journalctl -u microvm@agent-01.service --no-pager "
          "| grep -q 'harness-roundtrip-FAIL:'"
      )
      if rc == 0:
          raise Exception("agent emitted harness-roundtrip-FAIL")

      print("fleet-harness-signed-roundtrip: verify_artifact accepted the signed fixture")
    '';
  }
