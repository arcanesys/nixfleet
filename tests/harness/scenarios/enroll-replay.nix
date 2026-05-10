# LOADBEARING: validates the SQL-layer fix for the token-nonce TOCTOU race
# between `token_seen()` and `record_token_nonce()`; pre-fix two concurrent
# enrolls with the same nonce could both mint a cert.
{
  pkgs,
  harnessLib,
  testCerts,
  signedFixture,
  cpPkg,
  cliPkg,
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
  enrollEnabledModule = {lib, ...}: {
    services.nixfleet-control-plane.trustFile =
      lib.mkForce "${orgRootKeyFixture}/trust.json";
    environment.etc = {
      "harness/org-root.pem".source = "${orgRootKeyFixture}/private.pem";
      "harness/ca.pem".source = "${testCerts}/ca.pem";
      "harness/agent-99-key.pem".source = "${agentKeypairs.agent-99}/private.pem";
    };
    environment.systemPackages = [pkgs.openssl pkgs.sqlite pkgs.jq cliPkg];
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
      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.succeed("mkdir -p /tmp/enroll-test")

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
      for line in mint_stderr.splitlines():
          if line.startswith("nonce: "):
              nonce = line.split(": ", 1)[1].strip()
              break
      assert nonce is not None, f"could not parse nonce from {mint_stderr!r}"
      print(f"step 3: minted token with nonce={nonce}")

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
      host.succeed("systemctl start nixfleet-control-plane.service")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.succeed(
          "nixfleet mint-token "
          "--hostname agent-99 "
          f"--csr-pubkey-fingerprint '{fp}' "
          "--org-root-key /etc/harness/org-root.pem "
          "--validity-hours 1 "
          "> /tmp/enroll-test/token-fresh.json "
          "2>/dev/null"
      )
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
