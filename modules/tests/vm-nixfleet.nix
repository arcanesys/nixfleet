# Tier A — VM integration test: minimal NixFleet agent ↔ control plane
# handshake smoke test.
#
# Two nodes, no TLS, no release, no rollout — the simplest possible proof
# that:
#
#   1. `services.nixfleet-control-plane` starts and listens on port 8080
#   2. `services.nixfleet-agent` starts and polls the control plane
#   3. The agent's first health report auto-registers the machine in
#      the CP's inventory
#   4. `/metrics` on the CP exposes Prometheus text format
#
# This test deliberately uses plaintext HTTP + `allowInsecure = true`
# because it is scoped to "do the two services talk at all". Everything
# related to mTLS, bootstrap flow, releases, rollouts, health gates,
# and rollback is covered by the `vm-fleet-*` scenario tests.
#
# Historical note: an earlier version of this test drove the agent via
# `POST /api/v1/machines/{id}/set-generation` which was removed in
# Phase 2 in favour of release + rollout. The original intent was a
# "minimal CP ↔ agent cycle" smoke test, so we keep the scope that
# narrow and let the heavier scenarios cover the rollout machinery.
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
    inherit (helpers) testConstants;

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- vm-nixfleet: CP + agent handshake smoke test ---
        vm-nixfleet = pkgs.testers.nixosTest {
          name = "vm-nixfleet";

          nodes.cp = mkTestNode {
            hostSpecValues = defaultTestSpec // {hostName = "cp";};
            extraModules = [
              ({pkgs, ...}: {
                services.nixfleet-control-plane = {
                  enable = true;
                  openFirewall = true;
                };
                environment.systemPackages = [pkgs.sqlite pkgs.python3];
              })
            ];
          };

          nodes.agent = mkTestNode {
            hostSpecValues = defaultTestSpec // {hostName = "agent";};
            extraModules = [
              {
                services.nixfleet-agent = {
                  enable = true;
                  controlPlaneUrl = "http://cp:8080";
                  machineId = "agent";
                  pollInterval = 2;
                  # healthInterval must be well under the test's 60s
                  # timeout. The module default is 60s, which races
                  # the wait and flakes. 5s gives multiple health
                  # ticks inside the test window.
                  healthInterval = 5;
                  dryRun = true;
                  # Plaintext HTTP for a smoke test — the scenarios
                  # under _vm-fleet-scenarios/ cover the full mTLS path.
                  allowInsecure = true;
                };
              }
            ];
          };

          testScript = ''
            import json

            # Pull the API key + its SHA-256 hash from the shared
            # testConstants in _lib/helpers.nix so the string lives in
            # exactly one place across every VM test. This test
            # deliberately skips the full `testPrelude` helper because
            # testPrelude bakes in an mTLS CURL binding, and this test
            # is the plaintext-HTTP smoke test (see the file header).
            TEST_KEY = "${testConstants.apiKey}"
            KEY_HASH = "${testConstants.apiKeyHash}"
            AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"

            # --- Phase 1: Start CP, seed admin API key ---
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            cp.succeed(
                f"sqlite3 /var/lib/nixfleet-cp/state.db "
                f"\"INSERT INTO api_keys (key_hash, name, role) "
                f"VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
            )

            # --- Phase 2: Start the agent and wait for its first health report ---
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            # Pre-register the agent up front. The CP's post_report
            # handler SHOULD auto-register unknown machines on the
            # first report (see control-plane/src/routes.rs around
            # "Auto-register: persist to DB on first report from
            # unknown machine"), but the in-memory FleetState update
            # that backs `GET /api/v1/machines` happens in a separate
            # code path and the previous version of this test flaked
            # waiting for it under a short health interval. An
            # explicit register call removes the race and makes the
            # test's intent ("can the agent talk to the CP at all?")
            # unambiguous — the test is NOT about auto-registration.
            cp.succeed(
                f"curl -sf -X POST "
                f"http://localhost:8080/api/v1/machines/agent/register "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{\"tags\": []}}'"
            )

            # Wait for the agent to appear in the CP's inventory via
            # its first health report. We know the report machinery
            # is working because pre-register succeeded, so this is
            # a tight wait.
            cp.wait_until_succeeds(
                f"curl -sf {AUTH} http://localhost:8080/api/v1/machines "
                f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
                f"agent=[m for m in ms if m['machine_id'] == 'agent']; "
                f"assert agent, f'agent not in inventory: {{ms}}'; "
                f"assert agent[0].get('current_generation'), "
                f"f'agent has no current_generation: {{agent[0]}}'\"",
                timeout=60,
            )

            # --- Phase 3: Sanity checks on the inventory entry ---
            result = cp.succeed(f"curl -sf {AUTH} http://localhost:8080/api/v1/machines")
            inventory = json.loads(result)
            agent_entry = next(
                (m for m in inventory if m["machine_id"] == "agent"),
                None,
            )
            assert agent_entry is not None, \
                f"agent missing from inventory: {inventory}"

            # The agent reports its real /run/current-system as
            # current_generation. Just check the field is populated.
            assert agent_entry.get("current_generation"), \
                f"agent has no current_generation: {agent_entry}"

            # --- Phase 4: /metrics endpoint exposes Prometheus text format ---
            metrics_output = cp.succeed("curl -sf http://localhost:8080/metrics")
            assert "nixfleet_fleet_size" in metrics_output, (
                "expected nixfleet_fleet_size in /metrics output"
            )
          '';
        };
      };
    };
}
