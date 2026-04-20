# Tier A - VM fleet test: 4-node TLS/mTLS fleet with rollout, health gates, pause/resume.
#
# Nodes: cp (control plane), web-01, web-02 (healthy agents), db-01 (unhealthy agent).
# TLS: Nix-generated CA + server/client certs - no allowInsecure.
# Rollout: canary on web tag (passes), all-at-once on db tag (pauses on health gate).
#
# Run: nix build .#checks.x86_64-linux.vm-fleet --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs inputs;};

    mkTestNode = helpers.mkTestNode {
      hostSpecModule = ../_shared/host-spec-module.nix;
    };
    defaultTestSpec = helpers.defaultTestSpec;

    mkCpNode = args:
      helpers.mkCpNode ({inherit mkTestNode defaultTestSpec;} // args);
    mkAgentNode = args:
      helpers.mkAgentNode ({inherit mkTestNode defaultTestSpec;} // args);
    testPrelude = helpers.testPrelude;

    # Shared modules for web agent nodes (nginx health endpoint + node exporter)
    webAgentModules = [
      {
        services.nginx = {
          enable = true;
          virtualHosts.default.locations."/health".return = "200 ok";
        };
        nixfleet.monitoring.nodeExporter = {
          enable = true;
          openFirewall = true;
        };
      }
    ];

    # Shared health check config for web agents
    webHealthChecks = {
      http = [
        {
          url = "http://localhost:80/health";
          expectedStatus = 200;
        }
      ];
    };

    # Build-time TLS certificates: use the shared fleet cert set so the
    # CP + web-01 + web-02 + db-01 system closures dedupe with any
    # `_vm-fleet-scenarios/*.nix` scenario that uses the same mkCpNode /
    # mkAgentNode shape. See `helpers.nix::sharedTestCerts` for the
    # rationale.
    testCerts = helpers.sharedTestCerts;
  in
    # Gated to x86_64-linux: NixOS VM tests only run on Linux.
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        vm-fleet = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-fleet";

          nodes.cp = mkCpNode {inherit testCerts;};

          nodes.web-01 = mkAgentNode {
            inherit testCerts;
            hostName = "web-01";
            tags = ["web"];
            healthChecks = webHealthChecks;
            extraAgentModules = webAgentModules;
          };

          nodes.web-02 = mkAgentNode {
            inherit testCerts;
            hostName = "web-02";
            tags = ["web"];
            healthChecks = webHealthChecks;
            extraAgentModules = webAgentModules;
          };

          nodes.db-01 = mkAgentNode {
            inherit testCerts;
            hostName = "db-01";
            tags = ["db"];
            healthChecks.http = [
              {
                url = "http://localhost:9999/health";
                expectedStatus = 200;
                timeout = 2;
              }
            ];
          };

          testScript = ''
            ${testPrelude {}}

            # --- Step 1: Boot CP + seed admin API key ---
            cp_boot_and_seed(cp)

            # --- Step 2: Register all agents with their tags ---
            for host, tags in [("web-01", ["web"]), ("web-02", ["web"]), ("db-01", ["db"])]:
                tags_json = json.dumps(tags)
                cp.succeed(
                    f"{CURL} -X POST {API}/api/v1/machines/{host}/register "
                    f"{AUTH} "
                    f"-H 'Content-Type: application/json' "
                    f"-d '{{\"tags\": {tags_json}}}'"
                )

            # --- Step 3: Start all agents, wait for services ---
            start_agents(web_01, web_02, db_01)

            # Wait for all 3 agents to report to the CP
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"assert len(machines) == 3, f'Expected 3 machines, got {{len(machines)}}'\"",
                timeout=60,
            )

            # --- Step 4: Canary rollout on web tag (should succeed) ---
            # Since the agents run in dryRun=true they do not actually switch,
            # but they still report nix::current_generation() which resolves
            # to /run/current-system. We therefore build the release whose
            # entries point at each agent's actual current toplevel - that
            # way the executor's generation gate (report.generation ==
            # release_entry.store_path) matches immediately and the rollout
            # can proceed to health evaluation normally.
            web_01_toplevel = web_01.succeed("readlink -f /run/current-system").strip()
            web_02_toplevel = web_02.succeed("readlink -f /run/current-system").strip()

            web_release_id = create_release(cp, [
                {"hostname": "web-01", "store_path": web_01_toplevel, "tags": ["web"]},
                {"hostname": "web-02", "store_path": web_02_toplevel, "tags": ["web"]},
            ])
            web_rollout_id = create_rollout(
                cp, web_release_id, "web",
                strategy="staged",
                batch_sizes=["1", "100%"],
                health_timeout=30,
            )
            wait_rollout_status(cp, web_rollout_id, "completed", timeout=120)

            # --- Step 5: Verify Prometheus metrics ---
            metrics = cp.succeed(f"{CURL} {API}/metrics")
            assert "nixfleet_fleet_size" in metrics, "Missing nixfleet_fleet_size in CP metrics"
            assert "nixfleet_rollouts_total" in metrics, "Missing nixfleet_rollouts_total in CP metrics"

            # Node exporter on web-01 should respond
            web_01.succeed("curl -sf http://localhost:9100/metrics | grep node_cpu")

            # --- Step 6: Rollout on db tag - health gate fails, rollout pauses ---
            # Same trick as step 4: release entry points at db-01's real
            # toplevel so the generation gate matches and evaluate_batch
            # proceeds to health evaluation, which then fails because
            # db-01's configured health check hits :9999 (nothing listening).
            db_01_toplevel = db_01.succeed("readlink -f /run/current-system").strip()
            db_release_id = create_release(cp, [
                {"hostname": "db-01", "store_path": db_01_toplevel, "tags": ["db"]},
            ])
            db_rollout_id = create_rollout(cp, db_release_id, "db", health_timeout=10)

            # Wait for rollout to pause (health check on port 9999 fails - nothing listening)
            wait_rollout_status(cp, db_rollout_id, "paused", timeout=60)

            # Resume the paused rollout and verify it transitions out of paused
            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/rollouts/{db_rollout_id}/resume {AUTH}"
            )
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/rollouts/{db_rollout_id} "
                f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
                f"assert r['status'] != 'paused', f'Still paused after resume'\"",
                timeout=30,
            )
          '';
        };
      };
    };
}
