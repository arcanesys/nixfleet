{
  lib,
  harnessLib,
  testCerts,
  resolvedJsonPath,
  agentNames ? ["agent-01" "agent-02"],
  scenarioName ? "fleet-harness-smoke",
  ...
}: let
  cpHostModule = harnessLib.mkCpHostModule {
    inherit testCerts resolvedJsonPath;
  };

  mkAgent = name:
    harnessLib.mkAgentNode {
      inherit testCerts;
      hostName = name;
    };

  agents = lib.listToAttrs (map (n: {
      name = n;
      value = mkAgent n;
    })
    agentNames);

  # FOOTGUN: parallel cold-boot at N>2 saturates I/O on commodity HW; qemu user-net DHCP stops converging, route-wait hangs.
  isFleetN = builtins.length agentNames > 2;
  staggerSecs = 8;
in
  harnessLib.mkFleetScenario {
    name = scenarioName;
    inherit cpHostModule agents;
    agentVmAutostart = !isFleetN;
    timeout = 1200;
    testScript = ''
      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("harness-cp.service")
      host.wait_for_open_port(8443)

      ${
        if isFleetN
        then ''
          print("staggered start: launching ${toString (builtins.length agentNames)} microvms with ${toString staggerSecs}s gap")
          for idx, vm in enumerate(${builtins.toJSON agentNames}):
              host.execute(f"systemctl start --no-block microvm@{vm}.service")
              if idx < len(${builtins.toJSON agentNames}) - 1:
                  time.sleep(${toString staggerSecs})
          for vm in ${builtins.toJSON agentNames}:
              host.wait_for_unit(f"microvm@{vm}.service", timeout=300)
        ''
        else ''
          host.wait_for_unit("microvms.target", timeout=300)
          for vm in ${builtins.toJSON agentNames}:
              host.wait_for_unit(f"microvm@{vm}.service", timeout=300)
        ''
      }

      deadline = time.monotonic() + max(300, 150 + 60 * len(${builtins.toJSON agentNames}))
      pending = set(${builtins.toJSON agentNames})
      while pending and time.monotonic() < deadline:
          done = set()
          for agent in pending:
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
          budget = max(300, 150 + 60 * len(${builtins.toJSON agentNames}))
          raise Exception(f"agents did not report harness-agent-ok within {budget}s: {pending}")

      print("fleet-harness-smoke: all agents fetched fleet.resolved.json over mTLS")
    '';
  }
