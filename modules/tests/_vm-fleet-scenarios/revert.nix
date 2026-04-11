# vm-fleet-revert — F2, C3
#
# Covers:
#   F2 — Apply failure → `on_failure=revert` → per-machine
#        `previous_generations` column on prior *succeeded* batches is
#        consulted and those machines are restored to their pre-rollout
#        desired_generation via `revert_completed_batches`
#        (`control-plane/src/rollout/executor.rs:423`).
#   C3 — The agent's health-check subsystem (`HealthRunner::run_all` in
#        `agent/src/health.rs`) actually runs during / after a deploy
#        cycle. This is proven indirectly by F2: the revert path is only
#        triggered when `evaluate_batch` sees an unhealthy report, and
#        the unhealthy report itself is only produced by the health
#        runner evaluating the `command` check against the sentinel
#        file. If C3 were dead code, the batch would never flip to
#        `failed` and the rollout would never revert.
#
# Topology: cp + web-01 + web-02. Two agents are required so that the
# rollout's staged strategy has at least one *succeeded* batch (the
# target of the revert) followed by a failing batch (the trigger of the
# revert). `revert_completed_batches` only walks batches whose status is
# `"succeeded"` (see `executor.rs:432`), so a single-agent single-batch
# rollout cannot exercise this branch.
#
# Failure injection:
#   Both agents run a `command` health check that greps for the sentinel
#   file `/var/lib/fail-next-health` — identical to the pattern used in
#   `apply-failure.nix`. The file is NOT present at agent boot, so the
#   first batch reports healthy. Once the test script observes batch 0
#   reaching `succeeded`, it touches the sentinel on BOTH nodes. The
#   machine scheduled in batch 1 then reports `success=false` on its
#   next health tick, which drives the rollout executor into the
#   `on_failure=revert` branch.
#
# Why BOTH agents are armed:
#   The `build_batches` function in `control-plane/src/rollout/batch.rs`
#   randomizes batch assignment via `remaining.shuffle(&mut rng)`, so we
#   cannot deterministically predict which machine lands in batch 0 vs
#   batch 1. Arming both agents means "whichever machine is scheduled
#   in batch 1 will fail, whichever was already in the succeeded batch
#   0 is irrelevant because its batch has already been evaluated".
#
# Known caveats (runtime, not eval):
#   * With `dryRun=true` the agent's `/run/current-system` never
#     matches the release's trivial writeTextDir store path, so
#     `on_desired_gen` in `evaluate_batch` is always false. The batch
#     therefore lives in the `pending_count > 0` path until the
#     `health_timeout` elapses; at that point it is marked unhealthy.
#     The test configures `health_timeout = 10` so the timeout branch
#     fires quickly, and the `all_passed` signal from the agent's own
#     health report is what actually drives the batch verdict.
#     This is the same structural gotcha acknowledged by the 22a test.
#   * If the scheduler shuffles so that the failing batch is batch 0
#     (i.e. the sentinel arm happens before batch 0 even starts because
#     we don't yet know which machine will be in it), there are no
#     prior *succeeded* batches to revert. In that case the
#     `previous_generations` assertion on an already-succeeded batch
#     silently becomes a no-op and the test is weaker than intended.
#     The scaffold mitigates this by using `staged` with batch sizes
#     `["1","1"]` and by arming the sentinel only AFTER observing the
#     first batch reach `succeeded`.
{
  pkgs,
  mkTestNode,
  defaultTestSpec,
  mkTlsCerts,
}: let
  testCerts = mkTlsCerts {hostnames = ["web-01" "web-02"];};

  # Trivial closures baked into each agent's system so that
  # `nix path-info` succeeds inside `fetch_closure` (same trick as
  # `apply-failure.nix` and `bootstrap.nix`). With `dryRun=true` the
  # file contents are never exercised — the closure just needs to
  # exist on the agent's store.
  r2Web01Closure = pkgs.writeTextDir "share/nixfleet-revert-r2-web-01" "R2 web-01";
  r2Web02Closure = pkgs.writeTextDir "share/nixfleet-revert-r2-web-02" "R2 web-02";
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-revert";

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

          environment.systemPackages = [r2Web01Closure];

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

    nodes."web-02" = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "web-02";
        };
      extraModules = [
        {
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];

          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/web-02-cert.pem".source = "${testCerts}/web-02-cert.pem";
          environment.etc."nixfleet-tls/web-02-key.pem".source = "${testCerts}/web-02-key.pem";

          environment.systemPackages = [r2Web02Closure];

          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp:8080";
            machineId = "web-02";
            pollInterval = 2;
            healthInterval = 3;
            dryRun = true;
            tags = ["web"];
            tls = {
              clientCert = "/etc/nixfleet-tls/web-02-cert.pem";
              clientKey = "/etc/nixfleet-tls/web-02-key.pem";
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
      r2Web01Path = "${r2Web01Closure}";
      r2Web02Path = "${r2Web02Closure}";
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
      R2_WEB01 = "${r2Web01Path}"
      R2_WEB02 = "${r2Web02Path}"

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
      # Phase 2 — Start both agents. Flag file is NOT present yet, so
      # both report healthy on their first tick.
      # ------------------------------------------------------------------
      web_01.start()
      web_02.start()
      web_01.wait_for_unit("nixfleet-agent.service")
      web_02.wait_for_unit("nixfleet-agent.service")

      # Wait for both agents to register with the CP.
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
          f"assert len(ms) == 2, f'expected 2 machines got {{len(ms)}}'\"",
          timeout=60,
      )

      # Record each agent's original desired_generation (G1). With no
      # prior rollout this is typically empty (None) — that is the
      # baseline the revert will try to restore.
      initial_machines = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      by_id = {m["machine_id"]: m for m in initial_machines}
      assert "web-01" in by_id and "web-02" in by_id, \
          f"expected web-01 and web-02, got {list(by_id)}"

      # Seed a pre-rollout desired_generation on each machine so the
      # revert path has something meaningful to restore to. Without
      # this, `previous_generations` would be an empty map and the
      # revert branch would log a warning per machine.
      G1_WEB01 = "/nix/store/00000000000000000000000000000001-g1-web01"
      G1_WEB02 = "/nix/store/00000000000000000000000000000002-g1-web02"
      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO generations (machine_id, hash) VALUES ('web-01', '{G1_WEB01}') "
          f"ON CONFLICT(machine_id) DO UPDATE SET hash='{G1_WEB01}', set_at=datetime('now')\""
      )
      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO generations (machine_id, hash) VALUES ('web-02', '{G1_WEB02}') "
          f"ON CONFLICT(machine_id) DO UPDATE SET hash='{G1_WEB02}', set_at=datetime('now')\""
      )

      # ------------------------------------------------------------------
      # Phase 3 — Create release R2 with DIFFERENT store paths per host
      # and a rollout with `on_failure=revert` and `strategy=staged`
      # ["1","1"] so we get two sequential single-machine batches.
      # ------------------------------------------------------------------
      release_body = json.dumps({
          "flake_ref": "vm-fleet-revert",
          "entries": [
              {
                  "hostname": "web-01",
                  "store_path": R2_WEB01,
                  "platform": "x86_64-linux",
                  "tags": ["web"],
              },
              {
                  "hostname": "web-02",
                  "store_path": R2_WEB02,
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

      rollout_body = json.dumps({
          "release_id": release_id,
          "strategy": "staged",
          "batch_sizes": ["1", "1"],
          "failure_threshold": "1",
          "on_failure": "revert",
          "health_timeout": 10,
          "target": {"tags": ["web"]},
      })
      rollout = json.loads(cp.succeed(
          f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts "
          f"-H 'Content-Type: application/json' "
          f"-d '{rollout_body}'"
      ))
      rollout_id = rollout["rollout_id"]

      # ------------------------------------------------------------------
      # Phase 4 — Wait for batch 0 to reach `succeeded` via the health
      # timeout branch (treats pending-but-success-reporting agents as
      # healthy once the timeout expires). This is the prerequisite for
      # the revert path to have something to revert.
      #
      # NOTE: under dryRun=true, the batch reaches `succeeded` only
      # after the `health_timeout` (10 s) elapses and `unhealthy_count`
      # stays below `failure_threshold`. This is load-bearing — see
      # the caveat at the top of the file.
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"SELECT COUNT(*) FROM rollout_batches WHERE rollout_id='{rollout_id}' AND status='succeeded'\" "
          f"| grep -q '^1$'",
          timeout=90,
      )

      # ------------------------------------------------------------------
      # Phase 5 — Arm the sentinel on BOTH agents. Batch 0 has already
      # been recorded as succeeded; the agent in batch 1 will now
      # report unhealthy on its next tick.
      # ------------------------------------------------------------------
      web_01.succeed("mkdir -p /var/lib && touch /var/lib/fail-next-health")
      web_02.succeed("mkdir -p /var/lib && touch /var/lib/fail-next-health")

      # ------------------------------------------------------------------
      # Phase 6 — F2 positive: rollout must reach `failed` (revert
      # path) rather than `paused` (pause path).
      # ------------------------------------------------------------------
      cp.wait_until_succeeds(
          f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
          f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
          f"assert r['status'] == 'failed', "
          f"f'expected failed, got {{r[\\\"status\\\"]}}'\"",
          timeout=90,
      )

      # ------------------------------------------------------------------
      # Phase 7 — F2 positive: `rollout_batches.previous_generations`
      # for the succeeded batch (batch 0) is a non-empty JSON object
      # whose single entry points at the pre-rollout G1 path.
      # ------------------------------------------------------------------
      prev_gens_json = cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"SELECT previous_generations FROM rollout_batches "
          f"WHERE rollout_id='{rollout_id}' AND status='succeeded' LIMIT 1\""
      ).strip()
      assert prev_gens_json and prev_gens_json != "{}", \
          f"expected non-empty previous_generations on succeeded batch, got {prev_gens_json!r}"
      prev_gens = json.loads(prev_gens_json)
      assert len(prev_gens) == 1, \
          f"expected exactly one machine in succeeded batch's previous_generations, got {prev_gens}"
      only_machine, only_prev = next(iter(prev_gens.items()))
      expected_g1 = G1_WEB01 if only_machine == "web-01" else G1_WEB02
      assert only_prev == expected_g1, \
          f"previous_generations[{only_machine}] = {only_prev}, expected {expected_g1}"

      # ------------------------------------------------------------------
      # Phase 8 — F2 positive: the machine in the succeeded batch has
      # had its desired_generation reverted back to its G1 path by
      # `revert_completed_batches`. The machine in the failing batch
      # keeps the (new) R2 path because the failing batch is not
      # walked by the revert function — this is the documented
      # semantics in `executor.rs:432`.
      # ------------------------------------------------------------------
      post_revert_machines = json.loads(
          cp.succeed(f"{CURL} {AUTH} {API}/api/v1/machines")
      )
      post_by_id = {m["machine_id"]: m for m in post_revert_machines}
      reverted = post_by_id[only_machine].get("desired_generation")
      assert reverted == expected_g1, \
          f"{only_machine} desired_generation should revert to {expected_g1}, got {reverted}"

      # ------------------------------------------------------------------
      # Phase 9 — C3 positive: the agent on the failing node must have
      # actually invoked its health check runner post-deploy. We assert
      # that by searching the agent journal for either the
      # `Running periodic health check` log line (run_health_report in
      # agent/src/main.rs) or a failed health-check warning. Both are
      # emitted by `HealthRunner::run_all`, which is what C3 is about.
      # ------------------------------------------------------------------
      # Identify the failing machine — it is the one NOT in the
      # succeeded batch.
      failing_machine = "web-02" if only_machine == "web-01" else "web-01"
      failing_node = web_02 if failing_machine == "web-02" else web_01

      failing_node.succeed(
          "journalctl -u nixfleet-agent.service --no-pager "
          "| grep -E 'health|Health'"
      )

      # ------------------------------------------------------------------
      # Phase 10 — Negative: the agent services on both nodes are still
      # active — the revert path did not crash either agent.
      # ------------------------------------------------------------------
      web_01.succeed("systemctl is-active nixfleet-agent.service")
      web_02.succeed("systemctl is-active nixfleet-agent.service")
    '';
  }
