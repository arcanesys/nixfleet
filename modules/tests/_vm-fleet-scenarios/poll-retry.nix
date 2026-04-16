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
  inputs,
  mkCpNode,
  mkAgentNode,
  testCerts,
  testPrelude,
  ...
}:
pkgs.testers.nixosTest {
  specialArgs = {inherit inputs;};
  name = "vm-fleet-poll-retry";

  nodes.cp = mkCpNode {inherit testCerts;};

  nodes.agent = mkAgentNode {
    inherit testCerts;
    hostName = "agent";
    pollInterval = 5;
    # Short retry interval so the VM test can observe multiple retry
    # cycles within a reasonable wall-clock budget.
    agentExtraConfig.retryInterval = 5;
  };

  testScript = ''
    ${testPrelude {}}

    # ------------------------------------------------------------------
    # Step 1 — Start the agent BEFORE the cp. The agent's first poll
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
    # Step 2 — Start the cp, seed the admin API key.
    # ------------------------------------------------------------------
    cp.start()
    cp.wait_for_unit("nixfleet-control-plane.service")
    cp.wait_for_open_port(8080)

    seed_admin_key(cp)

    # ------------------------------------------------------------------
    # Step 3 — Negative check: the cp is reachable, admin auth works,
    # but NO machines are registered yet. This proves the registration
    # we observe in step 4 happens strictly AFTER the cp came up.
    # ------------------------------------------------------------------
    machines_pre = cp.succeed(
        f"{CURL} {AUTH} {API}/api/v1/machines "
        f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
        f"print(len(ms))\""
    ).strip()
    assert machines_pre == "0", \
        f"expected zero machines before agent recovery, got {machines_pre}"

    # ------------------------------------------------------------------
    # Step 4 — Positive: the agent's retry loop recovers and the
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
    # Step 5 — Positive: the agent unit is STILL active after the
    # retry + recovery cycle. The auto-retry path did not crash the
    # daemon at any point.
    # ------------------------------------------------------------------
    status_post = agent.execute("systemctl is-active nixfleet-agent.service")[1].strip()
    assert status_post == "active", \
        f"agent unit unexpectedly not active after recovery: {status_post!r}"
  '';
}
