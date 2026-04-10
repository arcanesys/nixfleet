# Tier A — VM fleet test: 4-node TLS/mTLS fleet with rollout, health gates, pause/resume.
#
# Nodes: cp (control plane), web-01, web-02 (healthy agents), db-01 (unhealthy agent).
# TLS: Nix-generated CA + server/client certs — no allowInsecure.
# Rollout: canary on web tag (passes), all-at-once on db tag (pauses on health gate).
#
# Run: nix build .#checks.x86_64-linux.vm-fleet --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};
    mkTlsCerts = import ./_lib/tls-certs.nix {inherit pkgs lib;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;

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

    # Build-time TLS certificates: fleet CA + CP server cert + 3 agent client certs.
    # Deterministic — no runtime setup needed.
    testCerts = mkTlsCerts {hostnames = ["web-01" "web-02" "db-01"];};

    # Helper: build an agent node with TLS, tags, and optional extra modules.
    mkAgentNode = {
      hostName,
      tags,
      healthChecks ? {},
      extraAgentModules ? [],
    }:
      mkTestNode {
        hostSpecValues =
          defaultTestSpec
          // {
            inherit hostName;
          };
        extraModules =
          [
            {
              # Trust the fleet CA so the agent can verify the CP's TLS cert
              security.pki.certificateFiles = ["${testCerts}/ca.pem"];

              environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
              environment.etc."nixfleet-tls/${hostName}-cert.pem".source = "${testCerts}/${hostName}-cert.pem";
              environment.etc."nixfleet-tls/${hostName}-key.pem".source = "${testCerts}/${hostName}-key.pem";

              services.nixfleet-agent = {
                enable = true;
                controlPlaneUrl = "https://cp:8080";
                machineId = hostName;
                pollInterval = 2;
                healthInterval = 5;
                dryRun = true;
                inherit tags;
                tls = {
                  clientCert = "/etc/nixfleet-tls/${hostName}-cert.pem";
                  clientKey = "/etc/nixfleet-tls/${hostName}-key.pem";
                };
                inherit healthChecks;
              };
            }
          ]
          ++ extraAgentModules;
      };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        vm-fleet = pkgs.testers.nixosTest {
          name = "vm-fleet";

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

          nodes.web-01 = mkAgentNode {
            hostName = "web-01";
            tags = ["web"];
            healthChecks = webHealthChecks;
            extraAgentModules = webAgentModules;
          };

          nodes.web-02 = mkAgentNode {
            hostName = "web-02";
            tags = ["web"];
            healthChecks = webHealthChecks;
            extraAgentModules = webAgentModules;
          };

          nodes.db-01 = mkAgentNode {
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
            import json

            TEST_KEY = "test-admin-key"
            KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
            AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
            CURL = "curl -sf --cacert /etc/nixfleet-tls/ca.pem --cert /etc/nixfleet-tls/cp-cert.pem --key /etc/nixfleet-tls/cp-key.pem"
            API = "https://localhost:8080"

            # --- Phase 1: Start CP, bootstrap API key ---
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            cp.succeed(
                f"sqlite3 /var/lib/nixfleet-cp/state.db "
                f"\"INSERT INTO api_keys (key_hash, name, role) VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
            )

            # --- Phase 2: Register all agents with their tags ---
            for host, tags in [("web-01", ["web"]), ("web-02", ["web"]), ("db-01", ["db"])]:
                tags_json = json.dumps(tags)
                cp.succeed(
                    f"{CURL} -X POST {API}/api/v1/machines/{host}/register "
                    f"{AUTH} "
                    f"-H 'Content-Type: application/json' "
                    f"-d '{{\"tags\": {tags_json}}}'"
                )

            # --- Phase 3: Start all agents, wait for services ---
            web_01.start()
            web_02.start()
            db_01.start()

            web_01.wait_for_unit("nixfleet-agent.service")
            web_02.wait_for_unit("nixfleet-agent.service")
            db_01.wait_for_unit("nixfleet-agent.service")

            # Wait for all 3 agents to report to the CP
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"assert len(machines) == 3, f'Expected 3 machines, got {{len(machines)}}'\"",
                timeout=60,
            )

            # --- Phase 4: Canary rollout on web tag (should succeed) ---
            rollout_body = json.dumps({
                "generation_hash": "/nix/store/fake-web-generation",
                "strategy": "staged",
                "batch_sizes": ["1", "100%"],
                "failure_threshold": "1",
                "on_failure": "pause",
                "health_timeout": 30,
                "target": {"tags": ["web"]}
            })
            rollout_resp = cp.succeed(
                f"{CURL} -X POST {API}/api/v1/rollouts {AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{rollout_body}'"
            )
            web_rollout = json.loads(rollout_resp)
            web_rollout_id = web_rollout["rollout_id"]

            # Wait for the web rollout to complete — both agents are healthy
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/rollouts/{web_rollout_id} "
                f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
                f"assert r['status'] == 'completed', f'Expected completed, got {{r[\\\"status\\\"]}}' \"",
                timeout=120,
            )

            # --- Phase 5: Verify Prometheus metrics ---
            metrics = cp.succeed(f"{CURL} {API}/metrics")
            assert "nixfleet_fleet_size" in metrics, "Missing nixfleet_fleet_size in CP metrics"
            assert "nixfleet_rollouts_total" in metrics, "Missing nixfleet_rollouts_total in CP metrics"

            # Node exporter on web-01 should respond
            web_01.succeed("curl -sf http://localhost:9100/metrics | grep node_cpu")

            # --- Phase 6: Rollout on db tag — health gate fails, rollout pauses ---
            db_rollout_body = json.dumps({
                "generation_hash": "/nix/store/fake-db-generation",
                "strategy": "all_at_once",
                "failure_threshold": "0",
                "on_failure": "pause",
                "health_timeout": 10,
                "target": {"tags": ["db"]}
            })
            db_rollout_resp = cp.succeed(
                f"{CURL} -X POST {API}/api/v1/rollouts {AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{db_rollout_body}'"
            )
            db_rollout = json.loads(db_rollout_resp)
            db_rollout_id = db_rollout["rollout_id"]

            # Wait for rollout to pause (health check on port 9999 fails — nothing listening)
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/rollouts/{db_rollout_id} "
                f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
                f"assert r['status'] == 'paused', f'Expected paused, got {{r[\\\"status\\\"]}}' \"",
                timeout=60,
            )

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
