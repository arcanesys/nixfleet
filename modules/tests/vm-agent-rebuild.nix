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
