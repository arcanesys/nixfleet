# tests/harness/scenarios/smoke.nix
#
# Minimal smoke scenario: 1 CP microVM + 2 agent microVMs boot on a host
# VM. Each agent fetches /fleet.resolved.json from the CP over mTLS and
# logs `harness-agent-ok: signedAt=...`. Scenario asserts both agent units
# reach `active` state and both emit the OK marker within 60s.
#
# This is the substrate for every future Checkpoint 2 scenario (magic
# rollback, compliance gate, freshness refusal). When those land, copy
# this file, flip agent config (e.g. inject bad signature into the fixture
# for freshness-refusal), and assert the opposite outcome.
#
# TODO(5): once Stream B's multi-algorithm signing lands, sign the
# fixture at build time and assert the agent's verify path accepts it.
# TODO(5): once Stream C's p256 verify path lands, add a twin scenario
# that swaps the fixture's signature for a tampered one and asserts the
# agent refuses to apply.
{
  lib,
  harnessLib,
  testCerts,
  resolvedJsonPath,
  ...
}: let
  cpModule = harnessLib.mkCpNode {
    inherit testCerts resolvedJsonPath;
    hostName = "cp";
  };

  mkAgent = name:
    harnessLib.mkAgentNode {
      inherit testCerts;
      hostName = name;
    };

  # Extension: change this list to fleet-N by generating
  # `agent-${toString i}` for i in 1..N. Each extra agent adds one
  # microvm guest to the host VM; the fixture must list the hostname too.
  agentNames = ["agent-01" "agent-02"];

  nodes =
    {
      cp = {
        type = "cp";
        module = cpModule;
      };
    }
    // lib.listToAttrs (map (n: {
        name = n;
        value = {
          type = "agent";
          module = mkAgent n;
        };
      })
      agentNames);
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-smoke";
    inherit nodes;
    timeout = 600;
    testScript = ''
      start_all()

      # Bring the host VM up and wait for the microvm.target to converge.
      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("microvms.target", timeout=300)

      # Every declared VM becomes a `microvm@<name>.service` on the host.
      for vm in ${builtins.toJSON (["cp"] ++ agentNames)}:
          host.wait_for_unit(f"microvm@{vm}.service", timeout=300)

      # Give the CP stub a moment to bind :8443 inside its guest, then
      # assert each agent logged the OK marker. The agent unit is
      # oneshot+RemainAfterExit, so its success is equivalent to one
      # successful mTLS fetch of the fixture.
      import time
      deadline = time.monotonic() + 60
      pending = set(${builtins.toJSON agentNames})
      while pending and time.monotonic() < deadline:
          done = set()
          for agent in pending:
              # The microvm logs end up on the host journal tagged with
              # the unit name. Grep for the marker emitted by
              # tests/harness/nodes/agent.nix.
              rc, _ = host.execute(
                  f"journalctl -u microvm@{agent}.service --no-pager "
                  f"| grep -q 'harness-agent-ok:'"
              )
              if rc == 0:
                  done.add(agent)
          pending -= done
          if pending:
              time.sleep(2)

      if pending:
          raise Exception(f"agents did not report harness-agent-ok within 60s: {pending}")

      print("fleet-harness-smoke: all agents fetched fleet.resolved.json over mTLS")
    '';
  }
