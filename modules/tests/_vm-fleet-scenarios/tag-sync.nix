# vm-fleet-tag-sync
#
# The real nixfleet-agent binary reports its configured tags via the
# periodic health report. This test starts a 2-node fleet (cp + one
# tagged agent), waits for the agent to report, and asserts the CP's
# view of the machine's tags matches the NixOS-config-side declaration.
#
# This file is the canonical template for all other _vm-fleet-scenarios/*
# files. Copy this structure, replace the nodes block with your topology,
# and replace the testScript body with your steps.
{
  pkgs,
  inputs,
  mkCpNode,
  mkAgentNode,
  testCerts,
  testPrelude,
  ...
}:
pkgs.testers.runNixOSTest {
  node.specialArgs = {inherit inputs;};
  name = "vm-fleet-tag-sync";

  nodes.cp = mkCpNode {inherit testCerts;};

  nodes.tagged = mkAgentNode {
    inherit testCerts;
    hostName = "tagged";
    # THE SUBJECT OF THE TEST: these tags must reach the CP via the
    # agent's periodic report.
    tags = ["web" "canary" "eu-west"];
  };

  testScript = ''
    ${testPrelude {}}

    # --- Step 1: Start CP, seed the admin API key ---
    cp.start()
    cp.wait_for_unit("nixfleet-control-plane.service")
    cp.wait_for_open_port(8080)
    seed_admin_key(cp)

    # --- Step 2: Start the tagged agent; wait for it to register ---
    tagged.start()
    tagged.wait_for_unit("nixfleet-agent.service")

    # Wait until the CP sees exactly one machine.
    cp.wait_until_succeeds(
        f"{CURL} {AUTH} {API}/api/v1/machines "
        f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
        f"assert len(ms) == 1, f'expected 1 machine got {{len(ms)}}'; "
        f"assert ms[0]['machine_id'] == 'tagged'\"",
        timeout=60,
    )

    # --- Step 3: Verify tags propagated via the health report ---
    # Query the DB directly for the machine_tags rows (mirrors what the
    # HTTP handler reads in get_machines_by_tags).
    tags_output = cp.succeed(
        "sqlite3 /var/lib/nixfleet-cp/state.db "
        "\"SELECT tag FROM machine_tags WHERE machine_id='tagged' ORDER BY tag\""
    )
    actual_tags = sorted(t.strip() for t in tags_output.strip().splitlines() if t.strip())
    expected_tags = ["canary", "eu-west", "web"]
    assert actual_tags == expected_tags, \
        f"expected tags {expected_tags}, got {actual_tags}"

    # --- Step 4: Filtering by a declared tag returns the machine ---
    canary_machines = cp.succeed(
        f"{CURL} {AUTH} {API}/api/v1/machines "
        f"| python3 -c \"import sys,json; "
        f"ms=[m for m in json.load(sys.stdin) if 'canary' in m.get('tags', [])]; "
        f"print(','.join(m['machine_id'] for m in ms))\""
    ).strip()
    assert canary_machines == "tagged", \
        f"tag filter for 'canary' returned {canary_machines!r}, expected 'tagged'"

    # --- Step 5 (negative): A tag the agent did NOT declare must NOT appear ---
    prod_machines = cp.succeed(
        f"{CURL} {AUTH} {API}/api/v1/machines "
        f"| python3 -c \"import sys,json; "
        f"ms=[m for m in json.load(sys.stdin) if 'production' in m.get('tags', [])]; "
        f"print(len(ms))\""
    ).strip()
    assert prod_machines == "0", \
        f"'production' tag should not appear (agent did not declare it); got {prod_machines} matches"

    # Negative control 2: the agent did not declare 'db' either.
    db_tag_check = cp.succeed(
        "sqlite3 /var/lib/nixfleet-cp/state.db "
        "\"SELECT COUNT(*) FROM machine_tags WHERE machine_id='tagged' AND tag='db'\""
    ).strip()
    assert db_tag_check == "0", f"'db' tag leaked into machine_tags row: {db_tag_check}"
  '';
}
