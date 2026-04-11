# vm-fleet-poll-retry — F7
#
# Covers F7 — Agent retry: CP unreachable on first poll → agent retries at
# `retry_interval` → CP comes up → agent's next poll succeeds and the
# machine registers.
#
# Topology: cp + agent. The agent is started BEFORE the cp, so its very
# first poll attempt hits a closed port (connection refused) or otherwise
# fails. The agent's main loop in `agent/src/main.rs` handles this via
# `PollOutcome::Failed`, which logs "Initial poll failed, scheduling retry"
# and then reschedules the next poll at `retry_interval`. We then start
# the cp and assert the agent recovers and registers.
#
# Positive assertions:
#   1. Before the cp is started, the agent's journal contains the
#      retry-scheduling log line (proves the retry path fired at least
#      once).
#   2. After the cp starts, the agent registers within a generous
#      timeout (≥ 60s → multiple retryInterval cycles).
#   3. The agent unit is STILL active after registration — the retry
#      path did not crash the agent.
#
# Negative assertions:
#   1. Immediately after the cp starts and is reachable, but BEFORE we
#      wait for the agent to register, `/api/v1/machines` returns `[]`.
#      This proves the registration happened AFTER the cp came up, not
#      somehow inherited from prior state.
{
  pkgs,
  mkTestNode,
  defaultTestSpec,
  mkTlsCerts,
}: let
  testCerts = mkTlsCerts {hostnames = ["agent"];};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-poll-retry";

    nodes.cp = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "cp";
        };
      extraModules = [
        ({pkgs, ...}: {
          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/cp-cert.pem".source = "${testCerts}/cp-cert.pem";
          environment.etc."nixfleet-tls/cp-key.pem".source = "${testCerts}/cp-key.pem";

          services.nixfleet-control-plane = {
            enable = true;
            openFirewall = true;
            tls = {
              cert = "/etc/nixfleet-tls/cp-cert.pem";
              key = "/etc/nixfleet-tls/cp-key.pem";
              clientCa = "/etc/nixfleet-tls/ca.pem";
            };
          };
          environment.systemPackages = [pkgs.sqlite pkgs.python3];
        })
      ];
    };

    nodes.agent = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "agent";
        };
      extraModules = [
        {
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];

          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/agent-cert.pem".source = "${testCerts}/agent-cert.pem";
          environment.etc."nixfleet-tls/agent-key.pem".source = "${testCerts}/agent-key.pem";

          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp:8080";
            machineId = "agent";
            pollInterval = 5;
            # Short retry interval so the VM test can observe multiple
            # retry cycles within a reasonable wall-clock budget.
            retryInterval = 5;
            healthInterval = 5;
            dryRun = true;
            tls = {
              clientCert = "/etc/nixfleet-tls/agent-cert.pem";
              clientKey = "/etc/nixfleet-tls/agent-key.pem";
            };
          };
        }
      ];
    };

    testScript = ''
      TEST_KEY = "test-admin-key"
      KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
      AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
      CURL = (
          "curl -sf --cacert /etc/nixfleet-tls/ca.pem "
          "--cert /etc/nixfleet-tls/cp-cert.pem "
          "--key /etc/nixfleet-tls/cp-key.pem"
      )
      API = "https://localhost:8080"

      # ------------------------------------------------------------------
      # Phase 1 — Start the agent BEFORE the cp. The agent's first poll
      # will hit a closed port and fail.
      # ------------------------------------------------------------------
      agent.start()
      agent.wait_for_unit("nixfleet-agent.service")

      # Give the agent enough time to execute at least one initial poll
      # attempt and log the failure. The "Initial poll failed, scheduling
      # retry" line is emitted from the main loop in agent/src/main.rs
      # after `PollOutcome::Failed` (see agent/src/main.rs:153–156).
      agent.wait_until_succeeds(
          "journalctl -u nixfleet-agent.service --no-pager "
          "| grep -F 'Initial poll failed, scheduling retry'",
          timeout=60,
      )

      # Sanity: the agent unit is still active after the poll failure —
      # the retry path must NOT crash the daemon.
      status_pre = agent.execute("systemctl is-active nixfleet-agent.service")[1].strip()
      assert status_pre == "active", \
          f"agent unit unexpectedly not active after failed poll: {status_pre!r}"

      # ------------------------------------------------------------------
      # Phase 2 — Start the cp, seed the admin API key.
      # ------------------------------------------------------------------
      cp.start()
      cp.wait_for_unit("nixfleet-control-plane.service")
      cp.wait_for_open_port(8080)

      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO api_keys (key_hash, name, role) "
          f"VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
      )

      # ------------------------------------------------------------------
      # Phase 3 — Negative check: the cp is reachable, admin auth works,
      # but NO machines are registered yet. This proves the registration
      # we observe in Phase 4 happens strictly AFTER the cp came up.
      # ------------------------------------------------------------------
      machines_pre = cp.succeed(
          f"{CURL} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
          f"print(len(ms))\""
      ).strip()
      assert machines_pre == "0", \
          f"expected zero machines before agent recovery, got {machines_pre}"

      # ------------------------------------------------------------------
      # Phase 4 — Positive: the agent's retry loop recovers and the
      # machine registers. The generous timeout covers multiple
      # retryInterval cycles (5s each) plus one health report round.
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
          f"assert len(ms) == 1, f'expected 1 machine got {{len(ms)}}'; "
          f"assert ms[0]['machine_id'] == 'agent'\"",
          timeout=90,
      )

      # ------------------------------------------------------------------
      # Phase 5 — Positive: the agent unit is STILL active after the
      # retry + recovery cycle. The auto-retry path did not crash the
      # daemon at any point.
      # ------------------------------------------------------------------
      status_post = agent.execute("systemctl is-active nixfleet-agent.service")[1].strip()
      assert status_post == "active", \
          f"agent unit unexpectedly not active after recovery: {status_post!r}"
    '';
  }
