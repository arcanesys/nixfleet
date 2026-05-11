{
  harnessLib,
  testCerts,
  signedFixture,
  agenixFixture,
  cpPkg,
  agentPkg,
  closureHash,
  agentKeypairs,
  ...
}: let
  cpHostModule = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  preseedModule = harnessLib.convergencePreseedModule {inherit closureHash;};

  # LOADBEARING: emits `harness-decrypt-ok: bytes=N` so testScript verifies decrypt without piping plaintext through journal.
  decryptModule = {pkgs, ...}: {
    environment.etc = {
      "harness-secret/identity.txt".source = "${agenixFixture}/identity.txt";
      "harness-secret/secret.age".source = "${agenixFixture}/secret.age";
    };
    systemd.tmpfiles.rules = [
      "d /run/secrets 0700 root root -"
    ];
    systemd.services.decrypt-test-secret = {
      description = "Decrypt the harness test secret into /run/secrets";
      wantedBy = ["multi-user.target"];
      before = ["nixfleet-agent.service"];
      after = ["local-fs.target"];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        StandardOutput = "journal+console";
        StandardError = "journal+console";
        ExecStart = pkgs.writeShellScript "decrypt-test-secret" ''
          set -euo pipefail
          ${pkgs.age}/bin/age -d \
            -i /etc/harness-secret/identity.txt \
            -o /run/secrets/test-token \
            /etc/harness-secret/secret.age
          chmod 600 /run/secrets/test-token
          bytes=$(${pkgs.coreutils}/bin/stat -c %s /run/secrets/test-token)
          echo "harness-decrypt-ok: bytes=$bytes"
        '';
      };
    };
  };

  agent = harnessLib.mkRealAgentNode {
    inherit testCerts signedFixture agentPkg;
    hostName = "agent-01";
    pollIntervalSecs = 10;
    sshHostKey = "${agentKeypairs.agent-01}/private.openssh";
    extraModules = [preseedModule decryptModule];
  };
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-secret-hygiene";
    inherit cpHostModule;
    agents = {agent-01 = agent;};
    timeout = 600;
    testScript = ''
      import re

      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.wait_for_unit("microvms.target", timeout=300)
      host.wait_for_unit("microvm@agent-01.service", timeout=300)

      decrypt_re = re.compile(r"harness-decrypt-ok: bytes=(\d+)")
      deadline = time.monotonic() + 180
      match = None
      while time.monotonic() < deadline:
          rc, out = host.execute(
              "journalctl -u microvm@agent-01.service --no-pager"
          )
          if rc == 0:
              m = decrypt_re.search(out)
              if m:
                  match = m
                  break
          time.sleep(2)
      if match is None:
          raise Exception("agent did not emit harness-decrypt-ok marker within 180s")

      decrypted_bytes = int(match.group(1))
      expected_bytes = int(host.succeed(
          "stat -c %s ${agenixFixture}/plaintext.txt"
      ).strip())
      assert decrypted_bytes == expected_bytes, (
          f"decrypt produced {decrypted_bytes} bytes; "
          f"fixture plaintext is {expected_bytes} bytes"
      )
      print(f"decrypt unit landed {decrypted_bytes}-byte plaintext on agent")

      print("waiting 45s for checkin traffic to accumulate…")
      time.sleep(45)

      # `grep -aFf needle` reads plaintext from the file so it never
      # transits the host journal.
      needle = "${agenixFixture}/plaintext.txt"
      checks = [
          ("CP state.db", "cat /var/lib/nixfleet-cp/state.db 2>/dev/null"),
          ("CP audit.log", "cat /var/lib/nixfleet-cp/audit.log 2>/dev/null"),
          ("CP journal", "journalctl -u nixfleet-control-plane.service --no-pager"),
          ("CP /etc tree", "find /etc/nixfleet-cp -type f -exec cat {} +"),
      ]
      leaks = []
      for label, cmd in checks:
          rc, _ = host.execute(f"{cmd} | grep -aFq -f {needle}")
          if rc == 0:
              leaks.append(label)
      if leaks:
          raise Exception(
              f"plaintext leaked into CP-resident state: {leaks}"
          )

      print(
          "fleet-harness-secret-hygiene: CP disk + journal contain "
          "zero bytes of the agent-side plaintext (ARCHITECTURE.md §8)."
      )
    '';
  }
