# vm-fleet-bootstrap — D1
#
# End-to-end: the real `nixfleet bootstrap` CLI against a fresh CP obtains
# the first admin API key over mTLS. The returned key is then used to:
#
#   1. List machines (initially empty)
#   2. Wait for two real agents (web-01, web-02) to register
#   3. List machines (now 2 visible)
#   4. Register a release via the HTTP API (curl shortcut — the release
#      create CLI path is already exercised by vm-fleet-release/R1, and it
#      requires the nix-shim machinery; D1 is about the bootstrap flow)
#   5. POST a rollout targeting tag=web and wait for it to reach `completed`
#
# Negative: a second `nixfleet bootstrap` call must fail with a non-zero
# exit code (the CP returns 409 Conflict once any API key exists).
{
  pkgs,
  mkTestNode,
  defaultTestSpec,
  mkCpNode,
  mkAgentNode,
  mkTlsCerts,
  tlsCertsModule,
  ...
}: let
  testCerts = mkTlsCerts {hostnames = ["operator" "web-01" "web-02"];};

  nixfleetCli = pkgs.callPackage ../../../cli {};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-bootstrap";

    nodes.cp = mkCpNode {inherit testCerts;};

    # The operator node is a plain client running the nixfleet CLI.
    # It isn't a CP or an agent so it uses the low-level mkTestNode
    # directly plus the shared tlsCertsModule helper.
    nodes.operator = mkTestNode {
      hostSpecValues = defaultTestSpec // {hostName = "operator";};
      extraModules = [
        (tlsCertsModule {
          inherit testCerts;
          certPrefix = "operator";
        })
        {
          environment.systemPackages = [
            nixfleetCli
            pkgs.curl
            pkgs.jq
            pkgs.python3
          ];
        }
      ];
    };

    nodes."web-01" = mkAgentNode {
      inherit testCerts;
      hostName = "web-01";
      tags = ["web"];
    };

    nodes."web-02" = mkAgentNode {
      inherit testCerts;
      hostName = "web-02";
      tags = ["web"];
    };

    testScript = ''
      import json

      # ------------------------------------------------------------------
      # Phase 1 — Start CP with no seeded admin key
      # ------------------------------------------------------------------
      cp.start()
      cp.wait_for_unit("nixfleet-control-plane.service")
      cp.wait_for_open_port(8080)

      # Sanity: the api_keys table must be empty before bootstrap.
      key_count = cp.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db 'SELECT COUNT(*) FROM api_keys'"
      ).strip()
      assert key_count == "0", f"expected empty api_keys, got {key_count}"

      # ------------------------------------------------------------------
      # Phase 2 — operator runs `nixfleet bootstrap`
      # ------------------------------------------------------------------
      operator.start()
      operator.wait_for_unit("multi-user.target")

      bootstrap_stdout = operator.succeed(
          "bash -lc '"
          "export NIXFLEET_CP_URL=https://cp:8080 && "
          "export NIXFLEET_CA_CERT=/etc/nixfleet-tls/ca.pem && "
          "export NIXFLEET_CLIENT_CERT=/etc/nixfleet-tls/operator-cert.pem && "
          "export NIXFLEET_CLIENT_KEY=/etc/nixfleet-tls/operator-key.pem && "
          "nixfleet bootstrap --name test-admin"
          "'"
      )

      # The CLI prints several stdout lines (the key, then "Saved to ...").
      # Extract the first line starting with nfk-.
      api_key = ""
      for line in bootstrap_stdout.splitlines():
          stripped = line.strip()
          if stripped.startswith("nfk-"):
              api_key = stripped
              break
      assert api_key, f"no nfk- key in bootstrap stdout: {bootstrap_stdout!r}"

      # The admin key is now in the CP database.
      key_count_after = cp.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db 'SELECT COUNT(*) FROM api_keys'"
      ).strip()
      assert key_count_after == "1", \
          f"expected 1 key after bootstrap, got {key_count_after}"

      # Compose the shared curl prefix used for subsequent API calls
      # on the operator side.
      CURL_BASE = (
          "curl -sf --cacert /etc/nixfleet-tls/ca.pem "
          "--cert /etc/nixfleet-tls/operator-cert.pem "
          "--key /etc/nixfleet-tls/operator-key.pem"
      )
      AUTH = f"-H 'Authorization: Bearer {api_key}'"
      API = "https://cp:8080"

      # ------------------------------------------------------------------
      # Phase 3 — Initial fleet is empty
      # ------------------------------------------------------------------
      initial = operator.succeed(
          f"{CURL_BASE} {AUTH} {API}/api/v1/machines"
      )
      initial_machines = json.loads(initial)
      assert len(initial_machines) == 0, \
          f"expected 0 machines before agents start, got {len(initial_machines)}"

      # ------------------------------------------------------------------
      # Phase 4 — Start both agents and wait for them to register
      # ------------------------------------------------------------------
      web_01.start()
      web_02.start()
      web_01.wait_for_unit("nixfleet-agent.service")
      web_02.wait_for_unit("nixfleet-agent.service")

      # Poll from the OPERATOR node because CURL_BASE references the
      # operator's client cert at /etc/nixfleet-tls/operator-cert.pem —
      # that file doesn't exist on cp. Running the wait on cp would make
      # curl fail at TLS setup (empty stdout → JSONDecodeError), looking
      # like the agents never registered even when they did.
      #
      # This is also the correct real-workflow path: in production an
      # operator runs `nixfleet machines list` from their workstation,
      # not from inside the control plane.
      operator.wait_until_succeeds(
          f"{CURL_BASE} {AUTH} {API}/api/v1/machines "
          f"| python3 -c \"import sys,json; "
          f"ms=json.load(sys.stdin); "
          f"assert len(ms) == 2, f'expected 2 machines, got {{len(ms)}}'\"",
          timeout=60,
      )

      # The operator's `nixfleet machines list` sees the same 2 machines.
      machines_cli = operator.succeed(
          "bash -lc '"
          "export NIXFLEET_CP_URL=https://cp:8080 && "
          "export NIXFLEET_CA_CERT=/etc/nixfleet-tls/ca.pem && "
          "export NIXFLEET_CLIENT_CERT=/etc/nixfleet-tls/operator-cert.pem && "
          "export NIXFLEET_CLIENT_KEY=/etc/nixfleet-tls/operator-key.pem && "
          f"export NIXFLEET_API_KEY={api_key} && "
          "nixfleet machines list"
          "'"
      )
      assert "web-01" in machines_cli, \
          f"expected web-01 in CLI output, got: {machines_cli!r}"
      assert "web-02" in machines_cli, \
          f"expected web-02 in CLI output, got: {machines_cli!r}"

      # ------------------------------------------------------------------
      # Phase 5 — Create a release via the HTTP API
      #
      # We use curl directly to avoid re-exercising the nix-shim machinery
      # (already covered by vm-fleet-release/R1).
      #
      # The release entries MUST point at each agent's actual
      # /run/current-system toplevel. Agents run in dryRun=true so they
      # never actually switch, but they still report their real current
      # generation in health reports. The rollout executor's generation
      # gate requires `report.generation == release_entry.store_path`
      # before a batch can proceed to health evaluation — a mismatch
      # pauses the rollout (the whole point of generation gating).
      #
      # vm-fleet.nix uses the same trick (Phase 4 comment). Earlier
      # versions of this file used throwaway writeTextDir closures and
      # hit `status=paused` for exactly this reason.
      # ------------------------------------------------------------------
      web_01_toplevel = web_01.succeed("readlink -f /run/current-system").strip()
      web_02_toplevel = web_02.succeed("readlink -f /run/current-system").strip()

      release_body = json.dumps({
          "flake_ref": "vm-fleet-bootstrap",
          "entries": [
              {
                  "hostname": "web-01",
                  "store_path": web_01_toplevel,
                  "platform": "x86_64-linux",
                  "tags": ["web"],
              },
              {
                  "hostname": "web-02",
                  "store_path": web_02_toplevel,
                  "platform": "x86_64-linux",
                  "tags": ["web"],
              },
          ],
      })
      release_resp = operator.succeed(
          f"{CURL_BASE} {AUTH} -X POST {API}/api/v1/releases "
          f"-H 'Content-Type: application/json' "
          f"-d '{release_body}'"
      )
      release = json.loads(release_resp)
      release_id = release["id"]
      assert release_id.startswith("rel-"), \
          f"expected release id with rel- prefix, got {release_id}"

      # ------------------------------------------------------------------
      # Phase 6 — Create a rollout and wait for completion
      # ------------------------------------------------------------------
      rollout_body = json.dumps({
          "release_id": release_id,
          "strategy": "all_at_once",
          "failure_threshold": "0",
          "on_failure": "pause",
          "health_timeout": 60,
          "target": {"tags": ["web"]},
      })
      rollout_resp = operator.succeed(
          f"{CURL_BASE} {AUTH} -X POST {API}/api/v1/rollouts "
          f"-H 'Content-Type: application/json' "
          f"-d '{rollout_body}'"
      )
      rollout = json.loads(rollout_resp)
      rollout_id = rollout["rollout_id"]

      # Same reason as Phase 4: poll from operator, not cp — CURL_BASE
      # references operator cert paths.
      operator.wait_until_succeeds(
          f"{CURL_BASE} {AUTH} {API}/api/v1/rollouts/{rollout_id} "
          f"| python3 -c \"import sys,json; r=json.load(sys.stdin); "
          f"assert r['status'] == 'completed', "
          f"f'expected completed, got {{r[\\\"status\\\"]}}'\"",
          timeout=180,
      )

      # ------------------------------------------------------------------
      # Phase 7 (negative) — Second bootstrap call must fail with 409
      # ------------------------------------------------------------------
      operator.fail(
          "bash -lc '"
          "export NIXFLEET_CP_URL=https://cp:8080 && "
          "export NIXFLEET_CA_CERT=/etc/nixfleet-tls/ca.pem && "
          "export NIXFLEET_CLIENT_CERT=/etc/nixfleet-tls/operator-cert.pem && "
          "export NIXFLEET_CLIENT_KEY=/etc/nixfleet-tls/operator-key.pem && "
          "nixfleet bootstrap --name second-attempt"
          "'"
      )

      # There must still be exactly one key in the database.
      final_key_count = cp.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db 'SELECT COUNT(*) FROM api_keys'"
      ).strip()
      assert final_key_count == "1", \
          f"expected 1 key after failed second bootstrap, got {final_key_count}"
    '';
  }
