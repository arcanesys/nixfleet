# Tier A — VM integration test: NixFleet agent ↔ control plane cycle.
#
# Two-node nixosTest proving the full systemd service lifecycle:
#   1. Control plane starts and listens on port 8080
#   2. Agent starts and polls the control plane
#   3. Operator sets a desired generation via the CP API
#   4. Agent detects mismatch, runs dry-run cycle, reports back
#   5. CP inventory reflects the agent's report
#
# Auth model: agent endpoints (report, desired-generation) have no API key
# middleware — agents authenticate via mTLS at the transport layer. Admin
# endpoints require a Bearer token. The test bootstraps a test API key for
# admin operations (set-generation, list machines).
#
# Run: nix build .#checks.x86_64-linux.vm-nixfleet --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- vm-nixfleet: agent ↔ control plane end-to-end cycle ---
        vm-nixfleet = pkgs.testers.nixosTest {
          name = "vm-nixfleet";

          nodes.cp = mkTestNode {
            hostSpecValues =
              defaultTestSpec
              // {
                hostName = "cp";
              };
            extraModules = [
              ({pkgs, ...}: {
                services.nixfleet-control-plane = {
                  enable = true;
                  openFirewall = true;
                };
                environment.systemPackages = [pkgs.sqlite];
              })
            ];
          };

          nodes.agent = mkTestNode {
            hostSpecValues =
              defaultTestSpec
              // {
                hostName = "agent";
              };
            extraModules = [
              {
                services.nixfleet-agent = {
                  enable = true;
                  controlPlaneUrl = "http://cp:8080";
                  machineId = "agent";
                  pollInterval = 2;
                  dryRun = true;
                  allowInsecure = true;
                };
              }
            ];
          };

          testScript = ''
            import json

            TEST_KEY = "test-admin-key"
            # SHA256 of TEST_KEY, precomputed for sqlite insert
            KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
            AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"

            # 1. Start control plane and wait for readiness
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            # Bootstrap a test admin API key directly in the CP database
            cp.succeed(
                f"sqlite3 /var/lib/nixfleet-cp/state.db "
                f"\"INSERT INTO api_keys (key_hash, name, role) VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
            )

            # 2. Pre-register the agent machine (required — unregistered machines get 404)
            cp.succeed(
                f"curl -sf -X POST "
                f"http://localhost:8080/api/v1/machines/agent/register "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{}}'"
            )

            # 3. Start agent and wait for its service
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            # 4. Set a desired generation for the agent machine via CP API.
            #    Use a fake store path -- the agent will detect a mismatch with
            #    its real /run/current-system and enter the fetch/dry-run path.
            cp.succeed(
                f"curl -sf -X POST "
                f"http://localhost:8080/api/v1/machines/agent/set-generation "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{\"hash\": \"/nix/store/fake-test-generation\"}}'"
            )

            # 5. Wait for the agent to poll, detect mismatch, and report back.
            #    Agent cycle: Idle (2s sleep) -> Checking (reads /run/current-system,
            #    compares with desired) -> Fetching (no cache_url = no-op) ->
            #    dry-run branch -> Reporting (POST /report with success=true,
            #    message="dry-run: would apply") -> Idle.
            #    After report, the CP inventory will show "agent" in machine list.
            # Wait for the agent to report (system_state transitions from "unknown" to "ok")
            cp.wait_until_succeeds(
                f"curl -sf {AUTH} http://localhost:8080/api/v1/machines | grep '\"system_state\":\"ok\"'",
                timeout=60,
            )

            # 6. Verify the machine inventory contains the agent with expected state
            result = cp.succeed(f"curl -sf {AUTH} http://localhost:8080/api/v1/machines")
            inventory: list[dict] = json.loads(result)

            agent_entry: dict | None = None
            for entry in inventory:
                if entry["machine_id"] == "agent":
                    agent_entry = entry
                    break

            assert agent_entry is not None, f"Agent not found in inventory: {inventory}"

            # dry-run reports success=true, which maps to system_state "ok"
            assert agent_entry["system_state"] == "ok", (
                f"Expected system_state 'ok' (dry-run reports success), "
                f"got: {agent_entry['system_state']}"
            )

            # The desired generation should be the fake hash we set
            assert agent_entry["desired_generation"] == "/nix/store/fake-test-generation", (
                f"Expected desired_generation '/nix/store/fake-test-generation', "
                f"got: {agent_entry['desired_generation']}"
            )

            # 7. Verify /metrics endpoint returns Prometheus text format
            metrics_output = cp.succeed("curl -sf http://localhost:8080/metrics")
            assert "nixfleet_fleet_size" in metrics_output, (
                "Expected nixfleet_fleet_size in /metrics output"
            )
          '';
        };
      };
    };
}
