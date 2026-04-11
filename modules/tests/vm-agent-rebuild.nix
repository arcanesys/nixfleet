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
            import json

            TEST_KEY = "test-admin-key"
            KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
            AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
            CURL = "curl -sf --cacert /etc/nixfleet-tls/ca.pem --cert /etc/nixfleet-tls/cp-cert.pem --key /etc/nixfleet-tls/cp-key.pem"
            API = "https://localhost:8080"

            def create_release(entries_list):
                """POST /api/v1/releases and return the release id."""
                body = json.dumps({
                    "flake_ref": "vm-agent-rebuild",
                    "entries": entries_list,
                })
                resp = cp.succeed(
                    f"{CURL} {AUTH} -X POST {API}/api/v1/releases "
                    f"-H 'Content-Type: application/json' "
                    f"-d '{body}'"
                )
                return json.loads(resp)["id"]

            def create_rollout(release_id, tag):
                """POST /api/v1/rollouts for the given release+tag."""
                body = json.dumps({
                    "release_id": release_id,
                    "strategy": "all_at_once",
                    "failure_threshold": "1",
                    "on_failure": "pause",
                    "health_timeout": 30,
                    "target": {"tags": [tag]},
                })
                resp = cp.succeed(
                    f"{CURL} {AUTH} -X POST {API}/api/v1/rollouts "
                    f"-H 'Content-Type: application/json' "
                    f"-d '{body}'"
                )
                return json.loads(resp)["rollout_id"]

            # --- Phase 1: Start CP, bootstrap API key ---
            cp.start()
            cp.wait_for_unit("nixfleet-control-plane.service")
            cp.wait_for_open_port(8080)

            cp.succeed(
                f"sqlite3 /var/lib/nixfleet-cp/state.db "
                f"\"INSERT INTO api_keys (key_hash, name, role) VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
            )

            # Register agent with the test tag (needed for rollout targeting).
            cp.succeed(
                f"{CURL} -X POST {API}/api/v1/machines/agent/register "
                f"{AUTH} "
                f"-H 'Content-Type: application/json' "
                f"-d '{{\"tags\": [\"test\"]}}'"
            )

            # --- Phase 2: Start agent, wait for it to register on the CP ---
            agent.start()
            agent.wait_for_unit("nixfleet-agent.service")

            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"assert any(m['machine_id'] == 'agent' for m in machines), 'agent not registered'\"",
                timeout=60,
            )

            # --- Test B: No-cache, pre-seeded store path ---
            # The agent's own /run/current-system is guaranteed to be in
            # its local store. We build a release whose entry points at
            # that path, so when the executor sets desired_generation,
            # the agent's check loop sees `current == desired` and
            # short-circuits to "up-to-date" without ever calling
            # fetch_closure or apply_generation.
            current_gen = agent.succeed("readlink /run/current-system").strip()

            release_b = create_release([
                {
                    "hostname": "agent",
                    "store_path": current_gen,
                    "platform": "x86_64-linux",
                    "tags": ["test"],
                },
            ])
            create_rollout(release_b, "test")

            # Agent should report `current_generation == desired` back
            # to the CP within a couple of poll cycles (pollInterval=2).
            cp.wait_until_succeeds(
                f"{CURL} {AUTH} {API}/api/v1/machines "
                f"| python3 -c \"import sys,json; machines=json.load(sys.stdin); "
                f"agent = [m for m in machines if m['machine_id'] == 'agent'][0]; "
                f"assert agent.get('current_generation') == '{current_gen}', "
                f"f'Expected {current_gen}, got {{agent.get(\\\"current_generation\\\")}}'\"",
                timeout=30,
            )

            # --- Test C: Missing path guard ---
            # Build a release whose entry points at a fabricated store
            # path that does NOT exist anywhere, with no cache URL
            # configured on the agent. The agent must refuse to switch
            # and stay at its original generation.
            fake_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-fake"
            release_c = create_release([
                {
                    "hostname": "agent",
                    "store_path": fake_path,
                    "platform": "x86_64-linux",
                    "tags": ["test"],
                },
            ])
            create_rollout(release_c, "test")

            # Wait a few poll cycles — agent should NOT switch.
            import time
            time.sleep(10)

            # Agent's /run/current-system must still be the original.
            actual_gen = agent.succeed("readlink /run/current-system").strip()
            assert actual_gen == current_gen, \
                f"Agent switched unexpectedly! Expected {current_gen}, got {actual_gen}"

            # Verify the agent logged the missing-path error from
            # `fetch_closure` (agent/src/nix.rs → "store path {...}
            # not found locally and no cache URL configured").
            agent.succeed(
                "journalctl -u nixfleet-agent.service --no-pager "
                "| grep -q 'not found locally and no cache URL configured'"
            )
          '';
        };
      };
    };
}
