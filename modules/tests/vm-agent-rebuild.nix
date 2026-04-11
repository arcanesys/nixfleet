# Tier A — VM agent rebuild tests: verify the full fetch → apply → verify pipeline.
#
# Test B: No-cache — CP + agent. Closure pre-seeded in agent store.
#         Agent verifies path exists, reports up-to-date.
# Test C: Missing path guard — CP + agent. Non-existent store path, no cache URL.
#         Agent detects missing path, reports error, stays at old generation.
#
# Run: nix build .#checks.x86_64-linux.vm-agent-rebuild --no-link
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

    # Build-time TLS certs: fleet CA + CP server cert + agent client cert.
    testCerts =
      pkgs.runCommand "nixfleet-rebuild-test-certs" {
        nativeBuildInputs = [pkgs.openssl];
      } ''
        mkdir -p $out

        # Fleet CA
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/ca-key.pem -out $out/ca.pem -days 365 -nodes \
          -subj '/CN=nixfleet-rebuild-test-ca'

        # CP server cert
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/cp-key.pem -out $out/cp-csr.pem -nodes \
          -subj '/CN=cp' \
          -addext 'subjectAltName=DNS:cp,DNS:localhost'
        openssl x509 -req -in $out/cp-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
          -CAcreateserial -out $out/cp-cert.pem -days 365 \
          -copy_extensions copyall

        # Agent client cert
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
          -keyout $out/agent-key.pem -out $out/agent-csr.pem -nodes \
          -subj '/CN=agent'
        openssl x509 -req -in $out/agent-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
          -CAcreateserial -out $out/agent-cert.pem -days 365

        rm -f $out/*.csr.pem $out/*.srl
      '';

    # CP node: control plane with TLS
    cpNode = mkTestNode {
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

    # Agent node: non-dry-run agent with mTLS
    agentNode = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "agent";
        };
      extraModules = [
        ({pkgs, ...}: {
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];

          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/agent-cert.pem".source = "${testCerts}/agent-cert.pem";
          environment.etc."nixfleet-tls/agent-key.pem".source = "${testCerts}/agent-key.pem";

          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp:8080";
            machineId = "agent";
            pollInterval = 2;
            healthInterval = 5;
            dryRun = false;
            tags = ["test"];
            tls = {
              clientCert = "/etc/nixfleet-tls/agent-cert.pem";
              clientKey = "/etc/nixfleet-tls/agent-key.pem";
            };
          };

          environment.systemPackages = [pkgs.python3];
        })
      ];
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        vm-agent-rebuild = pkgs.testers.nixosTest {
          name = "vm-agent-rebuild";

          nodes.cp = cpNode;
          nodes.agent = agentNode;

          testScript = ''
            TEST_KEY = "test-admin-key"
            KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
            AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
            CURL = "curl -sf --cacert /etc/nixfleet-tls/ca.pem --cert /etc/nixfleet-tls/cp-cert.pem --key /etc/nixfleet-tls/cp-key.pem"
            API = "https://localhost:8080"

            def set_desired_generation(machine_id, store_path):
                """Seed the CP's `generations` table directly so the agent's
                next poll of /api/v1/machines/{id}/desired-generation returns
                store_path. This deliberately bypasses the release+rollout
                executor: this test targets the agent's run_deploy_cycle
                (check → fetch → apply → report) only, and the rollout
                executor's batch/health-gate/conflict state machine is
                covered by the vm-fleet-* scenario tests."""
                cp.succeed(
                    f"sqlite3 /var/lib/nixfleet-cp/state.db "
                    f"\"INSERT INTO generations (machine_id, hash) "
                    f"VALUES ('{machine_id}', '{store_path}') "
                    f"ON CONFLICT(machine_id) DO UPDATE SET hash='{store_path}', "
                    f"set_at=datetime('now')\""
                )

            # --- Phase 1: Start CP, seed admin API key, register agent ---
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            cp.succeed(
                f"sqlite3 /var/lib/nixfleet-cp/state.db "
                f"\"INSERT INTO api_keys (key_hash, name, role) "
                f"VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
            )

            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/machines/agent/register "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{\"tags\": [\"test\"]}}'"
            )

            # --- Phase 2: Start the agent and wait for it to post its
            # first report so the CP has a current_generation on file. ---
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; ms=json.load(sys.stdin); "
                f"agent=[m for m in ms if m['machine_id'] == 'agent'][0]; "
                f"assert agent.get('current_generation'), "
                f"f'agent has no current_generation yet: {{agent}}'\"",
                timeout=60,
            )

            # --- Test B: No-cache, pre-seeded store path ---
            # Seed the agent's own /run/current-system as the desired
            # generation. The agent's next poll sees `current == desired`
            # and takes the "Already at desired generation" branch
            # (agent/src/main.rs:269-277), sends a success report, and
            # never touches fetch_closure. This proves the agent handles
            # the trivial no-op case correctly.
            current_gen = agent.succeed("readlink /run/current-system").strip()
            set_desired_generation("agent", current_gen)

            # Wait for the agent to log the "Already at desired generation"
            # line, which is the load-bearing signal that the no-op branch
            # fired. It's guaranteed to appear within one poll interval
            # (pollInterval=2s) of the desired-generation update.
            agent.wait_until_succeeds(
                "journalctl -u nixfleet-agent.service --no-pager "
                "| grep -q 'Already at desired generation'",
                timeout=30,
            )

            # --- Test C: Missing path guard ---
            # Seed a fabricated store path that does NOT exist locally and
            # is not reachable via any cache (cacheUrl is not configured
            # on this agent). The agent's fetch_closure calls `nix
            # path-info <fake>` which fails, and the agent logs
            # "store path ... not found locally and no cache URL
            # configured" without advancing /run/current-system.
            fake_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-fake"
            set_desired_generation("agent", fake_path)

            # Give the agent several poll cycles to try the fake path.
            import time
            time.sleep(10)

            # /run/current-system must not have moved.
            actual_gen = agent.succeed("readlink /run/current-system").strip()
            assert actual_gen == current_gen, (
                f"agent switched unexpectedly after being told to deploy a "
                f"non-existent path: expected {current_gen}, got {actual_gen}"
            )

            # The agent must have logged the fetch_closure "not found
            # locally" error message from agent/src/nix.rs.
            agent.succeed(
                "journalctl -u nixfleet-agent.service --no-pager "
                "| grep -q 'not found locally and no cache URL configured'"
            )
          '';
        };
      };
    };
}
