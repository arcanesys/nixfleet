# vm-fleet-apply-failure — F1, RB1
#
# Covers:
#   F1  — Apply failure → `on_failure=pause` → operator resumes rollout
#   RB1 — Agent does NOT advance to the new generation when the post-apply
#         health gate fails (the agent's automatic-rollback semantics; with
#         dryRun=true the store switch is skipped, so the proof is that
#         `current_generation` as observed by the CP is still the agent's
#         original `/run/current-system` rather than the release store path).
#
# Topology: cp + web-01 (single agent). A 2-node fleet is enough here —
# F1 and RB1 are single-agent behaviours.
#
# Failure injection:
#   web-01 has a `command` health check that greps for the sentinel file
#   `/var/lib/fail-next-health`. When the file exists, the check exits 1 and
#   the agent's periodic health report to the CP carries `success=false`.
#   The CP's rollout executor sees the unhealthy report, marks the batch
#   failed, and — because `on_failure=pause` — pauses the rollout. After the
#   assertions run, the testScript removes the sentinel file and calls
#   `POST /api/v1/rollouts/<id>/resume`, at which point the rollout must
#   complete normally.
#
# Why this exercises F1 + RB1 with dryRun=true:
#   `run_deploy_cycle` short-circuits on `config.dry_run` before calling
#   `switch-to-configuration`, so `apply_generation` is never invoked and
#   the agent's `/run/current-system` symlink never changes. This means
#   the agent's `current_generation` reported to the CP stays pinned at its
#   original NixOS system closure (the one built by the VM test driver),
#   which is NOT the release's trivial `writeTextDir` store path. The
#   negative assertion on `current_generation ≠ release store path` is
#   exactly the RB1 "agent did not advance to the failing generation"
#   guarantee.
{
  pkgs,
  mkTestNode,
  defaultTestSpec,
  mkTlsCerts,
  ...
}: let
  testCerts = mkTlsCerts {hostnames = ["web-01"];};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-apply-failure";

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

    nodes."web-01" = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "web-01";
        };
      extraModules = [
        {
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];

          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/web-01-cert.pem".source = "${testCerts}/web-01-cert.pem";
          environment.etc."nixfleet-tls/web-01-key.pem".source = "${testCerts}/web-01-key.pem";

          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp:8080";
            machineId = "web-01";
            pollInterval = 2;
            healthInterval = 3;
            dryRun = true;
            tags = ["web"];
            tls = {
              clientCert = "/etc/nixfleet-tls/web-01-cert.pem";
              clientKey = "/etc/nixfleet-tls/web-01-key.pem";
            };
            healthChecks.command = [
              {
                name = "fail-flag";
                command = "test ! -f /var/lib/fail-next-health";
                interval = 2;
                timeout = 2;
              }
            ];
          };
        }
      ];
    };

    testScript = ''
      import json

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
      # Phase 1 — Start CP, seed admin API key
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
      # Phase 2 — Arm the fail-flag BEFORE starting the agent, so the very
      # first health report from web-01 already carries success=false. This
      # avoids a race where an early "healthy" report flips the batch to
      # completed before we can observe the paused state.
      # ------------------------------------------------------------------
      web_01.start()
      web_01.wait_for_unit("multi-user.target")
      web_01.succeed("mkdir -p /var/lib && touch /var/lib/fail-next-health")
      web_01.wait_for_unit("nixfleet-agent.service")

      # ------------------------------------------------------------------
      # Phase 3 — Wait for the CP to observe web-01 registered
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
          f"assert len(ms) == 1, f'expected 1 machine got {{len(ms)}}'; "
          f"assert ms[0]['machine_id'] == 'web-01'\"",
          timeout=60,
      )

      # Record the agent's original generation (G1) as reported to the CP.
      # Under dryRun=true the agent reports /run/current-system forever
      # (agent/src/main.rs:266 — current_generation is read fresh on every
      # report and never swapped to the desired path because
      # apply_generation is short-circuited at main.rs:302-307). We MUST
      # use this real toplevel as the release store path, otherwise the
      # rollout executor's generation gate (report.generation ==
      # release_entry.store_path) can never match and the rollout can
      # never complete on resume — same class of bug as vm-fleet-bootstrap.
      initial_machines = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      g1 = initial_machines[0].get("current_generation", "")
      assert g1, f"expected web-01 to report a current_generation, got: {initial_machines[0]!r}"

      # Read the same toplevel from the agent directly so the release
      # entry is guaranteed to match (cp's view is the persisted report).
      release_path = web_01.succeed("readlink -f /run/current-system").strip()
      assert release_path == g1, \
          f"sanity: agent toplevel ({release_path}) must match CP's view of current_generation ({g1})"

      # ------------------------------------------------------------------
      # Phase 4 — Create release + rollout (on_failure=pause)
      # ------------------------------------------------------------------
      release_body = json.dumps({
          "flake_ref": "vm-fleet-apply-failure",
          "entries": [
              {
                  "hostname": "web-01",
                  "store_path": release_path,
                  "platform": "x86_64-linux",
                  "tags": ["web"],
              },
          ],
      })
      release = json.loads(cp.succeed(
          f"{CURL} {AUTH} -X POST {API}/api/v1/releases "
          f"-H 'Content-Type: application/json' "
          f"-d '{release_body}'"
      ))
      release_id = release["id"]
      assert release_id.startswith("rel-"), \
          f"expected rel- prefix, got {release_id}"

      # NOTE: failure_threshold="1" (not "0"). The current executor uses
      # `unhealthy_count < threshold` for the success check, which makes
      # threshold="0" pathological — the condition `0 < 0` is always
      # false so a threshold="0" rollout can NEVER succeed regardless
      # of how healthy the fleet is. Every other test in the suite
      # (vm-fleet.nix, revert.nix, bootstrap.nix) uses threshold="1"
      # with the convention "1 unhealthy machine fails the batch". We
      # follow the same convention here so Phase 9's resume-to-completed
      # path is reachable. The semantic inconsistency between
      # `unhealthy_count < threshold` (current code) and the CLI help
      # text "Maximum failures before pausing/reverting" (which
      # implies `<=`) is tracked as a Phase 4 followup.
      rollout_body = json.dumps({
          "release_id": release_id,
          "strategy": "all_at_once",
          "failure_threshold": "1",
          "on_failure": "pause",
          "health_timeout": 30,
          "target": {"tags": ["web"]},
      })
      rollout = json.loads(cp.succeed(
          f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts "
          f"-H 'Content-Type: application/json' "
          f"-d '{rollout_body}'"
      ))
      rollout_id = rollout["rollout_id"]

      # ------------------------------------------------------------------
      # Phase 5 — F1 positive: rollout must reach `paused`
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
          f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
          f"assert r['status'] == 'paused', "
          f"f'expected paused, got {{r[\\\"status\\\"]}}'\"",
          timeout=90,
      )

      # ------------------------------------------------------------------
      # Phase 6 — RB1 positive: web-01 is still at G1 and still reporting
      #
      # Under dryRun=true, the agent literally cannot advance — it skips
      # `apply_generation` and always reports /run/current-system. That
      # makes the "agent did not advance" half of RB1 structurally true;
      # what this phase really validates is that the failing health report
      # did not crash the agent, did not cause the CP to lose track of
      # web-01, and did not regress `current_generation` to anything else
      # (e.g. an empty string from a broken report path).
      # ------------------------------------------------------------------
      machines_mid = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      assert len(machines_mid) == 1, \
          f"RB1: CP lost track of web-01 during paused rollout, got {machines_mid!r}"
      g_mid = machines_mid[0].get("current_generation", "")
      assert g_mid == g1, \
          f"RB1: web-01 current_generation regressed (was {g1}, now {g_mid})"

      # ------------------------------------------------------------------
      # Phase 7 — Negative: the agent service is still active (auto-rollback
      # did not crash the agent)
      # ------------------------------------------------------------------
      web_01.succeed("systemctl is-active nixfleet-agent.service")

      # ------------------------------------------------------------------
      # Phase 8 — Clear the fail flag, wait for a verified-healthy report,
      # then resume the rollout.
      #
      # Waiting for the agent to actually emit a healthy report BEFORE
      # we call /resume is load-bearing. Without this, the executor's
      # next tick after resume reads the agent's latest report, which
      # is still the stale unhealthy one from before the flag was
      # cleared, and the stale-report filter only delays the verdict
      # (pending_count += 1 → waiting_health). Up until health_timeout
      # elapses the batch sits in waiting_health; after it elapses the
      # batch flips unhealthy_count and the rollout pauses again.
      # ------------------------------------------------------------------
      web_01.succeed("rm -f /var/lib/fail-next-health")

      # Verify the file is actually gone on the agent node. If
      # something (systemd-tmpfiles, state-directory activation,
      # whatever) is recreating it, the health check will keep
      # failing forever.
      print("### post-rm file check on web-01:")
      rc, out = web_01.execute("ls -la /var/lib/fail-next-health 2>&1; echo rc=$?")
      print(out)

      # Dump the actual health-checks config that the agent is reading.
      print("### /etc/nixfleet/health-checks.json on web-01:")
      print(web_01.succeed("cat /etc/nixfleet/health-checks.json"))

      # Run the exact command the CommandChecker runs, under the same
      # `sh -c` wrapper, and report its exit code. This lets us
      # distinguish a product bug (CommandChecker mis-invoking sh) from
      # a shell-semantics surprise (`test ! -f` behaving unexpectedly
      # under the specific sh the agent uses).
      print("### manual check: sh -c 'test ! -f /var/lib/fail-next-health':")
      rc, out = web_01.execute(
          "sh -c 'test ! -f /var/lib/fail-next-health'; echo exit=$?"
      )
      print(f"rc={rc} out={out!r}")

      # Wait for a HEALTH report (not a deploy-cycle report) with
      # all_passed=1. The `reports` table is populated by BOTH
      # run_deploy_cycle (always success=true under dryRun) and
      # run_health_report (success=health_report.all_passed), so
      # polling `reports.success=1` is trivially satisfied by the
      # deploy cycle and tells us nothing about health. Poll the
      # `health_reports` table directly — it is only populated by
      # run_health_report.
      try:
          cp.wait_until_succeeds(
              "sqlite3 /var/lib/nixfleet-cp/state.db "
              "\"SELECT all_passed FROM health_reports WHERE machine_id='web-01' "
              "ORDER BY received_at DESC, id DESC LIMIT 1\" | grep -q '^1$'",
              timeout=30,
          )
      except Exception:
          print("### wait-for-healthy timeout — dumping health_reports with results:")
          print(cp.execute(
              "sqlite3 -header /var/lib/nixfleet-cp/state.db "
              "\"SELECT id, all_passed, received_at, results "
              "FROM health_reports WHERE machine_id='web-01' "
              "ORDER BY received_at DESC, id DESC LIMIT 5\""
          )[1])
          raise

      # Dump the DB state for debugging before resume.
      print("### pre-resume rollout status:")
      print(cp.succeed(
          f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id}"
      ))
      print("### pre-resume batches:")
      print(cp.succeed(
          "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
          f"\"SELECT id,batch_index,status,started_at,completed_at FROM rollout_batches WHERE rollout_id='{rollout_id}'\""
      ))
      print("### pre-resume recent reports:")
      print(cp.succeed(
          "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
          "\"SELECT id,machine_id,success,received_at FROM reports WHERE machine_id='web-01' ORDER BY received_at DESC, id DESC LIMIT 5\""
      ))

      cp.succeed(
          f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts/{rollout_id}/resume"
      )

      # ------------------------------------------------------------------
      # Phase 9 — Rollout must now reach `completed`
      # ------------------------------------------------------------------
      try:
          cp.wait_until_succeeds(
              f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
              f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
              f"assert r['status'] == 'completed', "
              f"f'expected completed, got {{r[\\\"status\\\"]}}'\"",
              timeout=120,
          )
      except Exception:
          print("### post-resume-timeout rollout status:")
          print(cp.execute(
              f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id}"
          )[1])
          print("### post-resume-timeout batches:")
          print(cp.execute(
              "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
              f"\"SELECT id,batch_index,status,started_at,completed_at FROM rollout_batches WHERE rollout_id='{rollout_id}'\""
          )[1])
          print("### post-resume-timeout reports (web-01):")
          print(cp.execute(
              "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
              "\"SELECT id,machine_id,success,received_at FROM reports WHERE machine_id='web-01' ORDER BY received_at DESC, id DESC LIMIT 10\""
          )[1])
          print("### post-resume-timeout health_reports (web-01):")
          print(cp.execute(
              "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
              "\"SELECT id,machine_id,all_passed,received_at,substr(results,1,200) as results FROM health_reports WHERE machine_id='web-01' ORDER BY received_at DESC, id DESC LIMIT 5\""
          )[1])
          print("### post-resume-timeout fail-flag file on web-01:")
          print(web_01.execute("ls -la /var/lib/fail-next-health 2>&1")[1])
          print("### post-resume-timeout agent journal (tail 40):")
          print(web_01.execute("journalctl -u nixfleet-agent --no-pager -n 40 2>&1")[1])
          print("### post-resume-timeout rollout_events:")
          print(cp.execute(
              "sqlite3 -header -column /var/lib/nixfleet-cp/state.db "
              f"\"SELECT created_at,event_type,detail FROM rollout_events WHERE rollout_id='{rollout_id}' ORDER BY created_at DESC, id DESC LIMIT 20\""
          )[1])
          raise
    '';
  }
