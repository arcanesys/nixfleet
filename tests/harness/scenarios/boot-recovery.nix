{
  harnessLib,
  testCerts,
  signedFixture,
  cpPkg,
  agentPkg,
  pkgs,
  closureHash,
  agentKeypairs,
  ...
}: let
  cpHostModule = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  staleDispatchJson = builtins.toJSON {
    closure_hash = "stale-harness-fake-closure-does-not-match-current-system";
    channel_ref = "stable@harness";
    rollout_id = "stable@harness";
    confirm_endpoint = "/v1/agent/confirm";
    dispatched_at = "2026-01-01T00:00:00Z";
  };

  preseedModule = harnessLib.convergencePreseedModule {inherit closureHash;};

  agentNode = harnessLib.mkRealAgentNode {
    inherit testCerts signedFixture agentPkg;
    hostName = "agent-01";
    pollIntervalSecs = 10;
    sshHostKey = "${agentKeypairs.agent-01}/private.openssh";
    extraModules = [
      preseedModule
      ({lib, ...}: {
        systemd.services.nixfleet-agent.serviceConfig.ExecStartPre = lib.mkBefore [
          (pkgs.writeShellScript "harness-stage-stale-dispatch" ''
            set -euo pipefail
            mkdir -p /var/lib/nixfleet-agent
            cat > /var/lib/nixfleet-agent/last_dispatched <<'EOF'
            ${staleDispatchJson}
            EOF
            chmod 0600 /var/lib/nixfleet-agent/last_dispatched
            echo "harness: staged stale last_dispatched for boot-recovery test"
          '')
        ];
      })
    ];
  };
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-boot-recovery";
    inherit cpHostModule;
    agents = {
      agent-01 = agentNode;
    };
    timeout = 600;
    testScript = ''
      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.wait_for_unit("microvms.target", timeout=300)
      host.wait_for_unit("microvm@agent-01.service", timeout=300)

      print("step 1: waiting for agent first checkin (post-recovery)...")
      deadline = time.monotonic() + 180
      checked_in = False
      while time.monotonic() < deadline:
          rc, _ = host.execute(
              "journalctl -u nixfleet-control-plane.service --no-pager "
              "| grep -E 'checkin received.*agent-01'"
          )
          if rc == 0:
              checked_in = True
              break
          time.sleep(2)
      assert checked_in, "agent never checked in within 90s"
      print("step 1: agent checked in, recovery hook has fired")

      print("step 2: checking agent journal for StaleClearedMismatch action...")
      rc, out = host.execute(
          "journalctl -u microvm@agent-01.service --no-pager "
          "| grep -E 'boot-recovery: cleared stale dispatch record|StaleClearedMismatch|current/dispatched mismatch'"
      )
      assert rc == 0, (
          f"expected boot-recovery clear-stale log line in agent journal; got: {out!r}"
      )
      print("step 2: recovery cleared the stale record as expected")

      print(
          "fleet-harness-boot-recovery: fire-and-forget boot-recovery hook "
          "ran before poll loop and cleared the stale dispatch record."
      )
    '';
  }
