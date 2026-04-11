# Tier A — VM agent rebuild test: verify the agent's missing-path guard.
#
# Scope: one negative scenario — the agent is told (via a real release +
# rollout) to deploy a fabricated store path that does NOT exist anywhere,
# with no cache URL configured. The agent's `fetch_closure` must log the
# "not found locally and no cache URL configured" error and MUST NOT
# advance `/run/current-system`.
#
# This is the only VM test that runs with `dryRun = false`, so it is the
# only one that exercises the real `fetch → apply → verify` code path end
# to end. Other fetch-path coverage is indirect (vm-fleet-release proves
# `nix copy` + harmonia; vm-fleet-bootstrap proves the happy-path report
# cycle). The "pre-seeded path + up-to-date report" case that used to live
# here was dropped as trivially duplicated by vm-nixfleet and vm-fleet-*.
#
# Uses the shared _lib/helpers.nix infrastructure (same as the
# _vm-fleet-scenarios/*.nix files) instead of inlining its own openssl
# cert generation and CP/agent service wiring. The only scenario-
# specific override is `dryRun = false` on mkAgentNode — every other
# test (including every _vm-fleet-scenarios subtest) runs dryRun because
# they only need to prove the reporting cycle.
#
# Run: nix build .#checks.x86_64-linux.vm-agent-rebuild --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;

    mkCpNode = args:
      helpers.mkCpNode ({inherit mkTestNode defaultTestSpec;} // args);
    mkAgentNode = args:
      helpers.mkAgentNode ({inherit mkTestNode defaultTestSpec;} // args);

    # Use the shared cert derivation so this test's CP / agent node
    # closures dedupe with the `_vm-fleet-scenarios/*.nix` scenarios
    # that use the same mkCpNode / mkAgentNode shape.
    testCerts = helpers.sharedTestCerts;
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        vm-agent-rebuild = pkgs.testers.nixosTest {
          name = "vm-agent-rebuild";

          nodes.cp = mkCpNode {inherit testCerts;};

          # This is the one place in the entire test suite where we
          # want dryRun = false — the test's load-bearing assertion
          # depends on the real fetch path being exercised.
          nodes.agent = mkAgentNode {
            inherit testCerts;
            hostName = "agent";
            tags = ["test"];
            dryRun = false;
          };

          testScript = ''
            import json

            ${helpers.testPrelude {}}

            # --- Phase 1: Start CP, seed admin API key, register agent ---
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            seed_admin_key(cp)

            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/machines/agent/register "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{\"tags\": [\"test\"]}}'"
            )

            # --- Phase 2: Start the agent and wait for it to post its
            # first report so the CP has current_generation on file. ---
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
                f"agent=[m for m in ms if m['machine_id'] == 'agent'][0]; "
                f"assert agent.get('current_generation'), "
                f"f'agent has no current_generation yet: {{agent}}'\"",
                timeout=120,
            )

            # Record the agent's original /run/current-system. The test's
            # load-bearing assertion is that this symlink does NOT move
            # even after the CP tells the agent to deploy a fake path.
            original_gen = agent.succeed("readlink /run/current-system").strip()

            # --- Phase 3: Missing path guard ---
            # Create a release whose entry points at a fabricated store
            # path that does NOT exist anywhere. The agent's cacheUrl is
            # not configured, so `fetch_closure` calls `nix path-info
            # <fake>` which fails with "not found locally and no cache
            # URL configured" and the agent refuses to advance.
            #
            # The release + rollout machinery is the only way to
            # populate the agent's desired_generation (the legacy
            # `set-generation` admin endpoint was removed in Phase 2);
            # the executor's batch state is not asserted by this test,
            # only the agent's behaviour in response.
            fake_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-fake"

            release_body = json.dumps({
                "flake_ref": "vm-agent-rebuild",
                "entries": [
                    {
                        "hostname": "agent",
                        "store_path": fake_path,
                        "platform": "x86_64-linux",
                        "tags": ["test"],
                    },
                ],
            })
            release = json.loads(cp.succeed(
                f"{CURL} {AUTH} -X POST {API}/api/v1/releases "
                f"-H 'Content-Type: application/json' "
                f"-d '{release_body}'"
            ))

            rollout_body = json.dumps({
                "release_id": release["id"],
                "strategy": "all_at_once",
                "failure_threshold": "0",
                "on_failure": "pause",
                "health_timeout": 30,
                "target": {"tags": ["test"]},
            })
            cp.succeed(
                f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts "
                f"-H 'Content-Type: application/json' "
                f"-d '{rollout_body}'"
            )

            # --- Phase 4: Wait for the agent to log the "not found
            # locally" error from fetch_closure. This is the load-bearing
            # signal that the agent's refuse-to-switch branch fired. ---
            agent.wait_until_succeeds(
                "journalctl -u nixfleet-agent.service --no-pager "
                "| grep -q 'not found locally and no cache URL configured'",
                timeout=60,
            )

            # --- Phase 5: /run/current-system must not have moved ---
            actual_gen = agent.succeed("readlink /run/current-system").strip()
            assert actual_gen == original_gen, (
                f"agent switched unexpectedly after being told to deploy a "
                f"non-existent path: expected {original_gen}, got {actual_gen}"
            )
          '';
        };
      };
    };
}
