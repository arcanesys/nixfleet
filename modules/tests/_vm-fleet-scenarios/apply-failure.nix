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
  inputs,
  mkCpNode,
  mkAgentNode,
  testCerts,
  testPrelude,
  ...
}:
pkgs.testers.nixosTest {
  specialArgs = {inherit inputs;};
  name = "vm-fleet-apply-failure";

  nodes.cp = mkCpNode {inherit testCerts;};

  nodes."web-01" = mkAgentNode {
    inherit testCerts;
    hostName = "web-01";
    tags = ["web"];
    healthInterval = 3;
    healthChecks.command = [
      {
        name = "fail-flag";
        command = "test ! -f /var/lib/fail-next-health";
        interval = 2;
        timeout = 2;
      }
    ];
  };

  testScript = ''
    ${testPrelude {}}

    # ------------------------------------------------------------------
    # Step 1 — Boot CP + seed admin key
    # ------------------------------------------------------------------
    cp_boot_and_seed(cp)

    # ------------------------------------------------------------------
    # Step 2 — Arm the fail-flag BEFORE starting the agent, so the very
    # first health report from web-01 already carries success=false. This
    # avoids a race where an early "healthy" report flips the batch to
    # completed before we can observe the paused state.
    # ------------------------------------------------------------------
    web_01.start()
    web_01.wait_for_unit("multi-user.target")
    web_01.succeed("mkdir -p /var/lib && touch /var/lib/fail-next-health")
    web_01.wait_for_unit("nixfleet-agent.service")

    # ------------------------------------------------------------------
    # Step 3 — Wait for the CP to observe web-01 registered
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
    # Step 4 — Create release + rollout (on_failure=pause).
    #
    # `health_timeout=30` is shorter than the canonical default (60)
    # so the paused verdict lands inside the test timeout budget.
    # ------------------------------------------------------------------
    release_id = create_release(cp, [{"hostname": "web-01", "store_path": release_path}])
    assert release_id.startswith("rel-"), \
        f"expected rel- prefix, got {release_id}"
    rollout_id = create_rollout(cp, release_id, "web", health_timeout=30)

    # ------------------------------------------------------------------
    # Step 5 — F1 positive: rollout must reach `paused`
    # ------------------------------------------------------------------
    cp.wait_until_succeeds(
        f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
        f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
        f"assert r['status'] == 'paused', "
        f"f'expected paused, got {{r[\\\"status\\\"]}}'\"",
        timeout=90,
    )

    # ------------------------------------------------------------------
    # Step 6 — RB1 positive: web-01 is still at G1 and still reporting
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
    # Step 7 — Negative: the agent service is still active (auto-rollback
    # did not crash the agent)
    # ------------------------------------------------------------------
    web_01.succeed("systemctl is-active nixfleet-agent.service")

    # ------------------------------------------------------------------
    # Step 8 — Clear the fail flag, wait for a verified-healthy report,
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

    # Wait for a HEALTH report (not a deploy-cycle report) with
    # `all_passed=1` to confirm the agent's health check has
    # picked up the fresh state BEFORE we call /resume. Polling
    # `reports.success` does not work here: the reports table is
    # populated by both run_deploy_cycle (which always sets
    # success=true under dryRun, message="dry-run: would apply")
    # and run_health_report (success=health_report.all_passed).
    # Only the health_reports table is exclusive to the health
    # runner, so it is the load-bearing signal.
    cp.wait_until_succeeds(
        "sqlite3 /var/lib/nixfleet-cp/state.db "
        "\"SELECT all_passed FROM health_reports WHERE machine_id='web-01' "
        "ORDER BY received_at DESC, id DESC LIMIT 1\" | grep -q '^1$'",
        timeout=30,
    )

    cp.succeed(
        f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts/{rollout_id}/resume"
    )

    # ------------------------------------------------------------------
    # Step 9 — Rollout must now reach `completed`
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
