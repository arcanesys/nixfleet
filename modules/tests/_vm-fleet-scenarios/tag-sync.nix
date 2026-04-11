# vm-fleet-tag-sync — M3
#
# The real nixfleet-agent binary reports its configured tags via the
# periodic health report. This test starts a 2-node fleet (cp + one
# tagged agent), waits for the agent to report, and asserts the CP's
# view of the machine's tags matches the NixOS-config-side declaration.
#
# This file is the canonical template for all other _vm-fleet-scenarios/*
# files in Phase 3. Copy this structure, replace the nodes block with
# your topology, and replace the testScript body with your phases.
{
  pkgs,
  mkTestNode,
  defaultTestSpec,
  mkTlsCerts,
}: let
  testCerts = mkTlsCerts {hostnames = ["tagged"];};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-tag-sync";

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

    nodes.tagged = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "tagged";
        };
      extraModules = [
        {
          # Trust the fleet CA
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];

          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/tagged-cert.pem".source = "${testCerts}/tagged-cert.pem";
          environment.etc."nixfleet-tls/tagged-key.pem".source = "${testCerts}/tagged-key.pem";

          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp:8080";
            machineId = "tagged";
            pollInterval = 2;
            healthInterval = 5;
            dryRun = true;
            # THE SUBJECT OF THE TEST: these tags must reach the CP
            # via the agent's periodic report.
            tags = ["web" "canary" "eu-west"];
            tls = {
              clientCert = "/etc/nixfleet-tls/tagged-cert.pem";
              clientKey = "/etc/nixfleet-tls/tagged-key.pem";
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

      # --- Phase 1: Start CP, seed the admin API key ---
      cp.start()
      cp.wait_for_unit("nixfleet-control-plane.service")
      cp.wait_for_open_port(8080)

      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO api_keys (key_hash, name, role) VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
      )

      # --- Phase 2: Start the tagged agent; wait for it to register ---
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

      # --- Phase 3: Verify tags propagated via the health report ---
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

      # --- Phase 4: Filtering by a declared tag returns the machine ---
      canary_machines = cp.succeed(
          f"{CURL} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; "
          f"ms=[m for m in json.load(sys.stdin) if 'canary' in m.get('tags', [])]; "
          f"print(','.join(m['machine_id'] for m in ms))\""
      ).strip()
      assert canary_machines == "tagged", \
          f"tag filter for 'canary' returned {canary_machines!r}, expected 'tagged'"

      # --- Phase 5 (negative): A tag the agent did NOT declare must NOT appear ---
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
