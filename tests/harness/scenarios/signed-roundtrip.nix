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

      deadline = time.monotonic() + 180
      while time.monotonic() < deadline:
          rc, _ = host.execute(
              "journalctl -u microvm@agent-01.service --no-pager "
              "| grep -q 'harness-roundtrip-ok:'"
          )
          if rc == 0:
              break
          time.sleep(2)
      else:
          raise Exception("agent did not emit harness-roundtrip-ok within 180s")

      rc, _ = host.execute(
          "journalctl -u microvm@agent-01.service --no-pager "
          "| grep -q 'harness-roundtrip-FAIL:'"
      )
      if rc == 0:
          raise Exception("agent emitted harness-roundtrip-FAIL")

      print("fleet-harness-signed-roundtrip: verify_artifact accepted the signed fixture")
    '';
  }
