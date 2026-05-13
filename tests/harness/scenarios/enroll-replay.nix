# LOADBEARING: validates the SQL-layer fix for the token-nonce TOCTOU race
# between `token_seen()` and `record_token_nonce()`; pre-fix two concurrent
# enrolls with the same nonce could both mint a cert.
{
  pkgs,
  harnessLib,
  testCerts,
  signedFixture,
  signedSeedKey,
  cpPkg,
  cliPkg,
  canonicalizePkg,
  orgRootKeyFixture,
  agentKeypairs,
  ...
}: let
  cpHostBase = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  # GOTCHA: default cp-real trust.json carries only ciReleaseKey; /v1/enroll
  # needs orgRootKey.current (else 500). Override the daemon's --trust-file
  # via the module option (cp-real.nix points it at signedFixture).
  #
  # Also stages the deterministic agent-99 private key so the test's CSR
  # uses the pubkey that signedFixture declared in hosts.agent-99.pubkey  -
  # required for the post-#43 CSR-vs-declared-pubkey binding check.
  #
  # nixfleet#96 added a signed bootstrap-nonces allowlist that gates
  # /v1/enroll: the CP refuses any nonce not present in a verified
  # `bootstrap-nonces.json` sidecar. The harness builds + signs the
  # sidecar inside the VM after `mint-token` produces the random nonce,
  # serves it over a local python http.server (mirroring the
  # revocationsSource pattern), then restarts the CP so its first
  # immediate poll tick primes the allowlist before the parallel enrolls.
  bootstrapNoncesDir = "/var/lib/harness-bootstrap-nonces";
  enrollEnabledModule = {lib, ...}: {
    services.nixfleet-control-plane.trustFile =
      lib.mkForce "${orgRootKeyFixture}/trust.json";
    services.nixfleet-control-plane.bootstrapNoncesSource = {
      artifactUrl = "http://127.0.0.1:9091/bootstrap-nonces.json";
      signatureUrl = "http://127.0.0.1:9091/bootstrap-nonces.json.sig";
    };

    environment.etc = {
      "harness/org-root.pem".source = "${orgRootKeyFixture}/private.pem";
      "harness/ca.pem".source = "${testCerts}/ca.pem";
      "harness/agent-99-key.pem".source = "${agentKeypairs.agent-99}/private.pem";
      "harness/ci-release-key.pem".source = "${signedSeedKey}/privkey.pem";
    };
    environment.systemPackages = [
      pkgs.openssl
      pkgs.sqlite
      pkgs.jq
      cliPkg
      canonicalizePkg
    ];

    systemd.tmpfiles.rules = [
      "d ${bootstrapNoncesDir} 0755 root root -"
    ];

    systemd.services.harness-bootstrap-nonces-server = {
      description = "Static HTTP server for the harness bootstrap-nonces sidecar";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      serviceConfig = {
        Type = "simple";
        ExecStart = "${pkgs.python3}/bin/python3 -m http.server 9091 --directory ${bootstrapNoncesDir} --bind 127.0.0.1";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    # First CP poll fires immediately at startup; the sidecar must be in
    # place before the CP comes up if we want the very first tick to
    # prime the allowlist. The test script writes the sidecar first,
    # then restarts the CP.
    systemd.services.nixfleet-control-plane.after = [
      "harness-bootstrap-nonces-server.service"
    ];
  };

  combinedHostModule = {
    imports = [cpHostBase enrollEnabledModule];
  };
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-enroll-replay";
    cpHostModule = combinedHostModule;
    agents = {};
    timeout = 300;
    testScript = ''
      import json
      from datetime import datetime, timezone

      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("harness-bootstrap-nonces-server.service")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.succeed("mkdir -p /tmp/enroll-test")

      def write_signed_bootstrap_nonces(nonce, hostname, expires_at_rfc3339):
          """Build, canonicalize, sign and stage a bootstrap-nonces.json
          sidecar inside the VM. Mirrors the Nix-time sign-bytes.nix flow
          (ed25519 of the canonical bytes with the seed-derived
          ciReleaseKey) so the CP's polling verify passes."""
          signed_at = (
              datetime.now(timezone.utc)
              .replace(microsecond=0)
              .isoformat()
              .replace("+00:00", "Z")
          )
          payload = {
              "schemaVersion": 1,
              "bootstrapNonces": [
                  {
                      "nonce": nonce,
                      "hostname": hostname,
                      "expiresAt": expires_at_rfc3339,
                  }
              ],
              "meta": {
                  "schemaVersion": 1,
                  "signedAt": signed_at,
                  "ciCommit": "0000000000000000000000000000000000000000",
                  "signatureAlgorithm": "ed25519",
              },
          }
          host.succeed(
              "cat > /tmp/enroll-test/bootstrap-nonces.raw.json <<'JSON'\n"
              + json.dumps(payload)
              + "\nJSON"
          )
          host.succeed(
              "nixfleet-canonicalize "
              "< /tmp/enroll-test/bootstrap-nonces.raw.json "
              "> ${bootstrapNoncesDir}/bootstrap-nonces.json"
          )
          host.succeed(
              "openssl pkeyutl -sign -rawin "
              "-inkey /etc/harness/ci-release-key.pem "
              "-in ${bootstrapNoncesDir}/bootstrap-nonces.json "
              "-out ${bootstrapNoncesDir}/bootstrap-nonces.json.sig"
          )
          siglen = host.succeed(
              "stat -c %s ${bootstrapNoncesDir}/bootstrap-nonces.json.sig"
          ).strip()
          assert siglen == "64", f"bad sig length: {siglen}"

      def primed_log_count():
          """Count of "bootstrap nonces primed" log lines so far. Used
          to wait for a NEW prime after a restart (the line is emitted
          exactly once per CP startup; the in-memory primed flag resets
          on each restart, so a fresh prime adds a new line). awk over
          grep so a zero count is exit 0, not 1."""
          out = host.succeed(
              "journalctl -u nixfleet-control-plane.service --no-pager "
              "| awk '/bootstrap nonces primed/ {n++} END {print n+0}'"
          ).strip()
          return int(out)

      def restart_cp_and_wait_for_primed():
          """The CP's bootstrap-nonces poll cadence is 60 s; restarting
          makes the first tick fire immediately so the test doesn't
          block on the cadence."""
          prev = primed_log_count()
          host.succeed("systemctl restart nixfleet-control-plane.service")
          host.wait_for_unit("nixfleet-control-plane.service")
          host.wait_for_open_port(8443)
          host.wait_until_succeeds(
              "test \"$(journalctl -u nixfleet-control-plane.service --no-pager "
              "| awk '/bootstrap nonces primed/ {n++} END {print n+0}')\" -gt "
              f"{prev}",
              timeout=60,
          )

      # agent-99's private key is staged from agentKeypairs; its pubkey is
      # baked into the signedFixture (hosts.agent-99.pubkey) so the
      # CP's CSR↔declared-pubkey binding check passes.
      print("step 1: stage agent-99 private key and build CSR...")
      host.succeed(
          "install -m 0600 /etc/harness/agent-99-key.pem "
          "/tmp/enroll-test/agent-99-key.pem"
      )
      host.succeed(
          "openssl req -new -key /tmp/enroll-test/agent-99-key.pem "
          "-out /tmp/enroll-test/agent-99-csr.pem "
          "-subj '/CN=agent-99'"
      )

      # CP fingerprints the raw 32-byte ed25519 pubkey (SPKI trailer),
      # sha256 + base64. Mirror that exactly.
      print("step 2: compute pubkey fingerprint (rcgen-compatible)...")
      host.succeed(
          "openssl req -in /tmp/enroll-test/agent-99-csr.pem "
          "-noout -pubkey > /tmp/enroll-test/agent-99-pub.pem"
      )
      host.succeed(
          "openssl pkey -pubin -in /tmp/enroll-test/agent-99-pub.pem "
          "-outform DER -out /tmp/enroll-test/agent-99-pub.spki.der"
      )
      host.succeed(
          "tail -c 32 /tmp/enroll-test/agent-99-pub.spki.der "
          "> /tmp/enroll-test/agent-99-pub.raw"
      )
      fp = host.succeed(
          "openssl dgst -sha256 -binary /tmp/enroll-test/agent-99-pub.raw "
          "| base64 -w0"
      ).strip()
      print(f"step 2: fingerprint={fp}")

      print("step 3: mint bootstrap token...")
      mint_rc, _ = host.execute(
          "nixfleet mint-token "
          "--hostname agent-99 "
          f"--csr-pubkey-fingerprint '{fp}' "
          "--org-root-key /etc/harness/org-root.pem "
          "--validity-hours 1 "
          "> /tmp/enroll-test/token.json "
          "2> /tmp/enroll-test/mint.stderr"
      )
      if mint_rc != 0:
          stderr_dump = host.succeed("cat /tmp/enroll-test/mint.stderr || true")
          raise Exception(
              f"nixfleet mint-token failed (rc={mint_rc}). stderr:\n{stderr_dump}"
          )
      mint_stderr = host.succeed("cat /tmp/enroll-test/mint.stderr")
      nonce = None
      expires_at = None
      for line in mint_stderr.splitlines():
          if line.startswith("nonce: "):
              nonce = line.split(": ", 1)[1].strip()
          elif line.startswith("expiresAt: "):
              expires_at = line.split(": ", 1)[1].strip()
      assert nonce is not None, f"could not parse nonce from {mint_stderr!r}"
      assert expires_at is not None, f"could not parse expiresAt from {mint_stderr!r}"
      print(f"step 3: minted token with nonce={nonce}")

      print("step 3b: stage signed bootstrap-nonces sidecar and re-prime CP...")
      write_signed_bootstrap_nonces(nonce, "agent-99", expires_at)
      restart_cp_and_wait_for_primed()

      print("step 4: build EnrollRequest, fire two parallel posts...")
      host.succeed(
          "jq -n "
          "--slurpfile token /tmp/enroll-test/token.json "
          "--rawfile csr /tmp/enroll-test/agent-99-csr.pem "
          "'{token: $token[0], csrPem: $csr}' "
          "> /tmp/enroll-test/enroll.json"
      )

      # /v1/enroll is non-mTLS: host has no cert yet.
      host.succeed(
          "set +e; "
          "(curl -sk -o /dev/null -w '%{http_code}' "
          "  --cacert /etc/harness/ca.pem "
          "  -H 'Content-Type: application/json' "
          "  -d @/tmp/enroll-test/enroll.json "
          "  https://localhost:8443/v1/enroll "
          "  > /tmp/enroll-test/code1.txt) & "
          "(curl -sk -o /dev/null -w '%{http_code}' "
          "  --cacert /etc/harness/ca.pem "
          "  -H 'Content-Type: application/json' "
          "  -d @/tmp/enroll-test/enroll.json "
          "  https://localhost:8443/v1/enroll "
          "  > /tmp/enroll-test/code2.txt) & "
          "wait; "
          "set -e"
      )
      code1 = host.succeed("cat /tmp/enroll-test/code1.txt").strip()
      code2 = host.succeed("cat /tmp/enroll-test/code2.txt").strip()
      print(f"step 4: codes = ({code1}, {code2})")

      pair = sorted([code1, code2])
      assert pair == ["200", "409"], (
          f"expected exactly one 200 + one 409, got {pair} "
          f"(two-200 = race fix regression; other = unexpected)"
      )
      print("step 5: race outcome correct (exactly one 200, one 409)")

      row_count = host.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"SELECT COUNT(*) FROM token_replay WHERE nonce='{nonce}';\""
      ).strip()
      assert row_count == "1", (
          f"expected exactly 1 token_replay row for nonce={nonce}, got {row_count}"
      )
      print("step 6: token_replay has exactly one row for the nonce")

      host.succeed(
          "journalctl -u nixfleet-control-plane.service --no-pager "
          "| grep -F "
          "'enroll: token replay detected at record (concurrent enroll race or retry)'"
      )
      print("step 7: 'token replay detected at record' log line present")

      # Drop token_replay so the next enroll hits "no such table" (distinct
      # from ConstraintViolation), forcing the Err -> 500 arm.
      print("edge case: silent-record-failure -> !200 contract...")
      host.succeed("systemctl stop nixfleet-control-plane.service")
      host.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db "
          "'DROP TABLE token_replay;'"
      )

      # Mint a fresh token (new random nonce) and re-stage the allowlist
      # so the post-restart CP serves the new nonce.
      host.succeed(
          "nixfleet mint-token "
          "--hostname agent-99 "
          f"--csr-pubkey-fingerprint '{fp}' "
          "--org-root-key /etc/harness/org-root.pem "
          "--validity-hours 1 "
          "> /tmp/enroll-test/token-fresh.json "
          "2> /tmp/enroll-test/mint-fresh.stderr"
      )
      mint_fresh_stderr = host.succeed("cat /tmp/enroll-test/mint-fresh.stderr")
      fresh_nonce = None
      fresh_expires_at = None
      for line in mint_fresh_stderr.splitlines():
          if line.startswith("nonce: "):
              fresh_nonce = line.split(": ", 1)[1].strip()
          elif line.startswith("expiresAt: "):
              fresh_expires_at = line.split(": ", 1)[1].strip()
      assert fresh_nonce is not None, (
          f"could not parse fresh nonce from {mint_fresh_stderr!r}"
      )
      assert fresh_expires_at is not None, (
          f"could not parse fresh expiresAt from {mint_fresh_stderr!r}"
      )
      write_signed_bootstrap_nonces(fresh_nonce, "agent-99", fresh_expires_at)
      restart_cp_and_wait_for_primed()

      host.succeed(
          "jq -n "
          "--slurpfile token /tmp/enroll-test/token-fresh.json "
          "--rawfile csr /tmp/enroll-test/agent-99-csr.pem "
          "'{token: $token[0], csrPem: $csr}' "
          "> /tmp/enroll-test/enroll-fresh.json"
      )

      rc, fresh_code = host.execute(
          "curl -sk -o /dev/null -w '%{http_code}' "
          "--cacert /etc/harness/ca.pem "
          "-H 'Content-Type: application/json' "
          "-d @/tmp/enroll-test/enroll-fresh.json "
          "https://localhost:8443/v1/enroll"
      )
      assert rc == 0, f"fresh-nonce curl failed: {fresh_code}"
      assert fresh_code.strip() != "200", (
          f"fail-open regression: enroll returned 200 with broken "
          f"token_replay table (expected non-200). got {fresh_code!r}"
      )
      print(f"edge case: fresh-nonce enroll on broken table returned {fresh_code.strip()} (not 200, contract holds)")

      print(
          "fleet-harness-enroll-replay: race fix holds - concurrent "
          "/v1/enroll on same nonce yields exactly one 200 + one 409, "
          "exactly one token_replay row, log line present; broken "
          "token_replay table fails closed (not 200)."
      )
    '';
  }
