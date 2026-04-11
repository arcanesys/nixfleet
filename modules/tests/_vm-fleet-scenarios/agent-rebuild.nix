# vm-fleet-agent-rebuild — real `fetch → apply → verify` pipeline.
#
# The agent is told (via a real release + rollout) to deploy a
# fabricated store path that does NOT exist anywhere, with no cache
# URL configured. The agent's `fetch_closure` must log the "not found
# locally and no cache URL configured" error and MUST NOT advance
# `/run/current-system`.
#
# This is the one place in the suite where `dryRun = false` — every
# other scenario runs `dryRun = true` because they only need to prove
# the reporting cycle. Deleting this scenario would lose the only VM
# coverage of the real `fetch → apply → verify` code path. Indirect
# fetch-path coverage still exists:
#   * `vm-fleet-release` proves `nix copy` + harmonia end-to-end.
#   * `vm-fleet-bootstrap` proves the happy-path report cycle.
{
  pkgs,
  mkCpNode,
  mkAgentNode,
  testCerts,
  testPrelude,
  ...
}:
pkgs.testers.nixosTest {
  name = "vm-fleet-agent-rebuild";

  nodes.cp = mkCpNode {inherit testCerts;};

  # The agent runs with dryRun = false here. Every other scenario in
  # the suite uses dryRun = true.
  nodes.agent = mkAgentNode {
    inherit testCerts;
    hostName = "agent";
    tags = ["test"];
    dryRun = false;
  };

  testScript = ''
    ${testPrelude {}}

    # --- Step 1: Boot CP + seed admin API key + register agent ---
    cp_boot_and_seed(cp)

    cp.succeed(
        f"{CURL} -X POST {API}/api/v1/machines/agent/register "
        f"{AUTH} "
        f"-H 'Content-Type: application/json' "
        f"-d '{{\"tags\": [\"test\"]}}'"
    )

    # --- Step 2: Start the agent and wait for its first report so the
    # CP has current_generation on file. ---
    agent.start()
    agent.wait_for_unit("nixfleet-agent.service")

    cp.wait_until_succeeds(
        f"{CURL} {AUTH} {API}/api/v1/machines "
        f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
        f"agent=[m for m in ms if m['machine_id'] == 'agent'][0]; "
        f"assert agent.get('current_generation'), "
        f"f'agent has no current_generation yet: {{agent}}'\"",
        timeout=120,
    )

    # Record the agent's original /run/current-system. The test's
    # load-bearing assertion is that this symlink does NOT move
    # even after the CP tells the agent to deploy a fake path.
    original_gen = agent.succeed("readlink /run/current-system").strip()

    # --- Step 3: Missing-path guard — create a release pointing at a
    # fabricated store path that does NOT exist, with no cache URL
    # configured. `fetch_closure` calls `nix path-info <fake>` which
    # fails with "not found locally and no cache URL configured" and
    # the agent refuses to advance. The executor's batch state is not
    # asserted by this test — only the agent's behaviour in response. ---
    fake_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-fake"

    release_id = create_release(cp, [
        {"hostname": "agent", "store_path": fake_path, "tags": ["test"]},
    ])
    create_rollout(cp, release_id, "test", health_timeout=30)

    # --- Step 4: Wait for the agent to log the "not found locally"
    # error from fetch_closure. This is the load-bearing signal that
    # the agent's refuse-to-switch branch fired. ---
    agent.wait_until_succeeds(
        "journalctl -u nixfleet-agent.service --no-pager "
        "| grep -q 'not found locally and no cache URL configured'",
        timeout=60,
    )

    # --- Step 5: /run/current-system must not have moved ---
    actual_gen = agent.succeed("readlink /run/current-system").strip()
    assert actual_gen == original_gen, (
        f"agent switched unexpectedly after being told to deploy a "
        f"non-existent path: expected {original_gen}, got {actual_gen}"
    )
  '';
}
