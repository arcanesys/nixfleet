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
}: let
  testCerts = mkTlsCerts {hostnames = ["web-01"];};

  # Trivial closure baked into the agent's system so `nix path-info`
  # succeeds inside fetch_closure (same pattern as bootstrap.nix). dryRun
  # skips switch-to-configuration — file contents are never exercised.
  web01Closure = pkgs.writeTextDir "share/nixfleet-apply-failure-web-01" "fail test web-01";
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

          # Bake the trivial closure so fetch_closure's nix path-info check
          # succeeds without needing a binary cache.
          environment.systemPackages = [web01Closure];

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

    testScript = let
      web01Path = "${web01Closure}";
    in ''
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
      RELEASE_PATH = "${web01Path}"

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
      # With dryRun=true this is /run/current-system of the VM itself — a
      # real NixOS closure, NOT the trivial release store path.
      initial_machines = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      g1 = initial_machines[0].get("current_generation", "")
      assert g1, f"expected web-01 to report a current_generation, got: {initial_machines[0]!r}"
      assert g1 != RELEASE_PATH, \
          f"precondition: web-01 current_generation must differ from release path, got {g1}"

      # ------------------------------------------------------------------
      # Phase 4 — Create release + rollout (on_failure=pause)
      # ------------------------------------------------------------------
      release_body = json.dumps({
          "flake_ref": "vm-fleet-apply-failure",
          "entries": [
              {
                  "hostname": "web-01",
                  "store_path": RELEASE_PATH,
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

      rollout_body = json.dumps({
          "release_id": release_id,
          "strategy": "all_at_once",
          "failure_threshold": "0",
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
      # Phase 6 — RB1 positive: web-01 is still at G1, NOT the release path
      #
      # With dryRun=true the agent skips switch-to-configuration, so the
      # agent's `current_generation` as reported to the CP should still be
      # the original NixOS system closure (G1). This is the observable
      # signature that the failing deploy did not leave the machine stranded
      # on a half-applied generation — the rollback guarantee of RB1.
      # ------------------------------------------------------------------
      machines_mid = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      g_mid = machines_mid[0].get("current_generation", "")
      assert g_mid == g1, \
          f"RB1: web-01 current_generation changed despite failure (was {g1}, now {g_mid})"
      assert g_mid != RELEASE_PATH, \
          f"RB1: web-01 current_generation advanced to release path despite failure: {g_mid}"

      # ------------------------------------------------------------------
      # Phase 7 — Negative: the agent service is still active (auto-rollback
      # did not crash the agent)
      # ------------------------------------------------------------------
      web_01.succeed("systemctl is-active nixfleet-agent.service")

      # ------------------------------------------------------------------
      # Phase 8 — Clear the fail flag and resume the rollout
      # ------------------------------------------------------------------
      web_01.succeed("rm -f /var/lib/fail-next-health")

      cp.succeed(
          f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts/{rollout_id}/resume"
      )

      # ------------------------------------------------------------------
      # Phase 9 — Rollout must now reach `completed`
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
          f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
          f"assert r['status'] == 'completed', "
          f"f'expected completed, got {{r[\\\"status\\\"]}}'\"",
          timeout=120,
      )
    '';
  }
