# vm-fleet-release — R1, R2
#
# Exercises the REAL `cli::release::create` orchestration end-to-end:
#
#   R1: `nixfleet release create --push-to ssh://root@cache` builds a
#       canned closure, pushes it to a Harmonia binary cache via
#       `nix copy --to ssh://root@cache`, and registers the release
#       manifest with the control plane.
#   R2: An agent with `services.nixfleet-cache.enable = true` fetches the
#       release closure via substitution from `http://cache:5000`.
#
# Strategy: a shell `nix` shim (modules/tests/_lib/nix-shim.nix) intercepts
# `nix eval` / `nix build` / `nix flake metadata` subprocess calls from
# cli::release::create and returns canned JSON/text. `nix copy` is
# delegated to the real nix so the binary-cache transfer actually happens.
# The store path the shim returns is a real `pkgs.writeTextDir` derivation
# seeded into the builder node's closure at nixosTest build time.
{
  pkgs,
  lib,
  mkTestNode,
  defaultTestSpec,
  mkTlsCerts,
}: let
  testCerts = mkTlsCerts {hostnames = ["builder" "cache" "agent"];};

  # Real trivial closure that the shim returns from `nix build`. It exists
  # on the builder node's /nix/store because it's pulled in by the shim
  # derivation's closure (writeTextDir is a runtime dependency via string
  # interpolation in the shim's case statements).
  web01Closure = pkgs.writeTextDir "share/nixfleet-release-web-01" "hello from web-01";

  # Pre-generated throwaway test SSH keypair. Baking literal key material
  # into the test file avoids IFD (which nixosTest evaluation cannot do)
  # and also avoids rebuilding the key on every test run. This key has no
  # production value — it only authenticates the builder VM to the cache
  # VM inside the test network.
  testSshPublicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILjq8UWKdMurTHKPfL8+vESysUAR5gaBYH5X/QrSVp3a nixfleet-test-builder";

  # Use concatStringsSep to avoid indentation stripping rules in '' blocks.
  testSshPrivateKey = lib.concatStringsSep "\n" [
    "-----BEGIN OPENSSH PRIVATE KEY-----"
    "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW"
    "QyNTUxOQAAACC46vFFinTLq0xyj3y/PrxEsrFAEeYGgWB+V/0K0lad2gAAAJjM0bw7zNG8"
    "OwAAAAtzc2gtZWQyNTUxOQAAACC46vFFinTLq0xyj3y/PrxEsrFAEeYGgWB+V/0K0lad2g"
    "AAAEDV6C+WI9NR1F+Bmq4Y65IR8S7E6AlCKWGbBv9Nh6Nj9bjq8UWKdMurTHKPfL8+vESy"
    "sUAR5gaBYH5X/QrSVp3aAAAAFW5peGZsZWV0LXRlc3QtYnVpbGRlcg=="
    "-----END OPENSSH PRIVATE KEY-----"
    ""
  ];

  testSshPrivateKeyFile = pkgs.writeText "nixfleet-test-builder-key" testSshPrivateKey;

  # Cache signing key baked into the test closure as a store-path
  # derivation. harmonia.service's LoadCredential= resolves its source
  # at service start time; a store-path key is present from boot 0, so
  # no activation-script / user-creation ordering dance is needed. The
  # key is world-readable under /nix/store — that's only acceptable for
  # a throwaway test key. Production operators use agenix with
  # owner=harmonia mode=0400.
  testSigningKey = pkgs.runCommand "nixfleet-test-cache-signing-key" {} ''
    mkdir -p $out
    # nix-store --generate-binary-cache-key tries to touch
    # /nix/var/nix/profiles at startup, which is forbidden in the
    # build sandbox. Redirect its state dir into the build tmpdir so
    # it writes there instead.
    export NIX_STATE_DIR="$TMPDIR/nix-state"
    mkdir -p "$NIX_STATE_DIR"
    ${pkgs.nix}/bin/nix-store --generate-binary-cache-key \
      nixfleet-test-cache \
      $out/signing.secret \
      $out/signing.public
    chmod 0444 $out/signing.secret
  '';

  # Advertised public key for the cache client. The matching public
  # component would normally come from `testSigningKey`'s output, but
  # reading it back into eval would require IFD (which nixosTest
  # evaluation cannot do). The substitution step in the test uses
  # `--no-check-sigs` so this placeholder is never actually verified.
  cachePublicKeyPlaceholder = "nixfleet-test-cache:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

  nixShim = import ../_lib/nix-shim.nix {inherit pkgs lib;} {
    hosts = [
      {
        name = "web-01";
        platform = "x86_64-linux";
        tags = ["web"];
        storePath = "${web01Closure}";
      }
    ];
  };

  nixfleetCli = pkgs.callPackage ../../../cli {};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-release";

    nodes.cp = mkTestNode {
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

    nodes.cache = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "cache";
        };
      extraModules = [
        {
          services.openssh = {
            enable = true;
            settings = {
              PermitRootLogin = lib.mkForce "yes";
              PasswordAuthentication = lib.mkForce false;
            };
          };

          # Pre-seed the builder's public key in root's authorized_keys
          # so `nix copy --to ssh://root@cache` can push.
          users.users.root.openssh.authorizedKeys.keys = [testSshPublicKey];

          # Harmonia binary cache server (serves from local Nix store).
          #
          # The signing key is baked into the test closure as a Nix store
          # path (`testSigningKey` below, generated by a runCommand). This
          # sidesteps the systemd CREDENTIALS=243 failure mode we hit with
          # a /var/lib/nixfleet-cache/signing.secret activation-script
          # approach: the upstream services.harmonia.cache module uses
          # systemd's LoadCredential= to pass the signing key, which
          # resolves the source path at service start time. Any file that
          # depends on activation-script ordering races the service.
          #
          # A store-path key is present from boot 0, world-readable under
          # /nix/store (which is correct for a throwaway test key), and
          # requires no chown/chmod. Production operators use agenix with
          # an owner=harmonia mode=0400 secret; that's a separate code
          # path and not what this test exercises.
          services.nixfleet-cache-server = {
            enable = true;
            port = 5000;
            openFirewall = true;
            signingKeyFile = "${testSigningKey}/signing.secret";
          };

          # Trust incoming store paths from the builder (required for
          # `nix copy --to ssh://root@cache` when the paths are unsigned).
          nix.settings.trusted-users = ["root"];

          networking.firewall.allowedTCPPorts = [22 5000];
        }
      ];
    };

    nodes.builder = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "builder";
        };
      extraModules = [
        {
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];
          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/builder-cert.pem".source = "${testCerts}/builder-cert.pem";
          environment.etc."nixfleet-tls/builder-key.pem".source = "${testCerts}/builder-key.pem";

          environment.systemPackages = [
            nixfleetCli
            pkgs.openssh
            pkgs.jq
            # nixShim is installed as a regular package providing /bin/nix;
            # the sessionVariable below ensures it appears before the real
            # nix in PATH.
            nixShim
          ];

          # Prepend the shim's bin dir to PATH so `nix` resolves to the shim.
          environment.sessionVariables.PATH =
            lib.mkBefore ["${nixShim}/bin"];

          # Private SSH key to ssh into the cache node.
          environment.etc."ssh-builder-key" = {
            source = testSshPrivateKeyFile;
            mode = "0400";
          };
        }
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
          security.pki.certificateFiles = ["${testCerts}/ca.pem"];
          environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
          environment.etc."nixfleet-tls/agent-cert.pem".source = "${testCerts}/agent-cert.pem";
          environment.etc."nixfleet-tls/agent-key.pem".source = "${testCerts}/agent-key.pem";

          services.nixfleet-cache = {
            enable = true;
            cacheUrl = "http://cache:5000";
            publicKey = cachePublicKeyPlaceholder;
          };
        }
      ];
    };

    testScript = ''
      import json

      TEST_KEY = "test-admin-key"
      KEY_HASH = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9"
      CP_CURL_BUILDER = (
          "curl -sf --cacert /etc/nixfleet-tls/ca.pem "
          "--cert /etc/nixfleet-tls/builder-cert.pem "
          "--key /etc/nixfleet-tls/builder-key.pem "
          "-H 'Authorization: Bearer " + TEST_KEY + "' "
      )
      CP_CURL_LOCAL = (
          "curl -sf --cacert /etc/nixfleet-tls/ca.pem "
          "--cert /etc/nixfleet-tls/cp-cert.pem "
          "--key /etc/nixfleet-tls/cp-key.pem "
          "-H 'Authorization: Bearer " + TEST_KEY + "' "
      )

      # --- Phase 1: Start all nodes ---
      # Each wait has aggressive diagnostic output so a failure in any
      # step shows up in the nix build log instead of an opaque hang.
      print("### starting cp")
      cp.start()
      print("### starting cache")
      cache.start()
      print("### starting builder")
      builder.start()
      print("### starting agent")
      agent.start()

      print("### waiting for cp:nixfleet-control-plane")
      try:
          cp.wait_until_succeeds(
              "systemctl is-active nixfleet-control-plane.service", timeout=120
          )
      except Exception:
          print("=== cp:nixfleet-control-plane status ===")
          print(cp.execute("systemctl status nixfleet-control-plane.service --no-pager")[1])
          print("=== cp:nixfleet-control-plane journal ===")
          print(cp.execute("journalctl -u nixfleet-control-plane.service --no-pager -n 80")[1])
          raise
      cp.wait_for_open_port(8080)
      print("### cp:8080 open")

      print("### waiting for cache:sshd")
      cache.wait_for_unit("sshd.service")
      cache.wait_for_open_port(22)
      print("### cache:22 open")

      # Bounded wait for harmonia with a diagnostic dump on failure — the
      # upstream module uses systemd LoadCredential= and failures there
      # (exit 243) are silent to `wait_for_unit` which blocks forever.
      print("### waiting for cache:harmonia")
      try:
          cache.wait_until_succeeds(
              "systemctl is-active harmonia.service", timeout=120
          )
      except Exception:
          print("=== harmonia status ===")
          print(cache.execute("systemctl status harmonia.service --no-pager")[1])
          print("=== harmonia journal (last 120 lines) ===")
          print(cache.execute("journalctl -u harmonia.service --no-pager -n 120")[1])
          print("=== cache nix.conf ===")
          print(cache.execute("cat /etc/nix/nix.conf")[1])
          raise
      cache.wait_for_open_port(5000)
      print("### cache:5000 open — phase 1 complete")

      # Seed the admin API key on the CP via direct SQLite insert.
      cp.succeed(
          f"sqlite3 /var/lib/nixfleet-cp/state.db "
          f"\"INSERT INTO api_keys (key_hash, name, role) "
          f"VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
      )

      # The CLI is invoked with --flake /tmp/fake-flake. The CLI does not
      # validate the flake itself — only the shim reads the string — but
      # we create a harmless placeholder just in case.
      builder.succeed(
          "mkdir -p /tmp/fake-flake && "
          "printf '{ outputs = _: {}; }\\n' > /tmp/fake-flake/flake.nix"
      )

      # Install the builder's private key where ssh can pick it up.
      builder.succeed("mkdir -p /root/.ssh && chmod 700 /root/.ssh")
      builder.succeed("cp /etc/ssh-builder-key /root/.ssh/id_ed25519")
      builder.succeed("chmod 600 /root/.ssh/id_ed25519")
      # Pre-accept the cache's host key so `nix copy --to ssh://` does not
      # prompt. ssh-keyscan runs over the test network and populates
      # known_hosts.
      builder.succeed("ssh-keyscan -t ed25519 cache >> /root/.ssh/known_hosts")

      # Sanity: the shim must be first on PATH. It is a writeShellApplication
      # installed as a normal package; systemPackages + sessionVariables
      # ordering should ensure the shim resolves first.
      which_nix = builder.succeed(
          "bash -lc 'command -v nix'"
      ).strip()
      # The shim is installed under /nix/store/...-nix/bin/nix. We don't
      # assert the exact path; we assert it is NOT the default system nix
      # at /run/current-system/sw/bin/nix.
      assert which_nix != "/run/current-system/sw/bin/nix", \
          f"shim not prepended to PATH, got {which_nix!r}"

      # Pre-flight smoke tests — isolate which subprocess is broken
      # before invoking the CLI end-to-end. The previous run showed
      # nix copy exiting 124 with EMPTY captured output and NO new
      # ssh sessions recorded in the cache journal, which means nix
      # copy hung CLIENT-SIDE before even attempting the SSH leg.
      # Diagnose with a chain of ever-more-specific checks.

      print("### preflight: nix --version")
      print(builder.succeed("/run/current-system/sw/bin/nix --version"))

      print("### preflight: local store sees ${web01Closure}")
      print(builder.succeed(
          "/run/current-system/sw/bin/nix-store -q --references "
          + "${web01Closure}"
      ))

      print("### smoke: ssh -o BatchMode=yes root@cache true")
      rc, out = builder.execute(
          "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new "
          "-o ConnectTimeout=10 root@cache true",
          timeout=30,
      )
      if rc != 0:
          print("=== ssh smoke failed ===")
          print(out)
          raise Exception(f"ssh root@cache true returned {rc}")

      # Simpler than `nix copy`: just ask the remote store if it's
      # reachable. If THIS hangs, nix's ssh-store backend is broken
      # on the builder. If it succeeds but copy hangs, the issue is
      # somewhere deeper in the copy path.
      print("### smoke: nix store ping --store ssh://root@cache")
      builder.succeed(
          "timeout 60 /run/current-system/sw/bin/nix store ping "
          "--store ssh://root@cache "
          "> /tmp/nix-ping.log 2>&1 || true"
      )
      print(builder.succeed("cat /tmp/nix-ping.log"))
      ping_rc = int(builder.succeed(
          "timeout 60 /run/current-system/sw/bin/nix store ping "
          "--store ssh://root@cache >/dev/null 2>&1; echo $?"
      ).strip())
      if ping_rc != 0:
          raise Exception(f"nix store ping returned {ping_rc}")

      # Now the real nix-copy smoke. Run with -vvv and redirect to a
      # file so buffered output survives a SIGKILL from timeout.
      # `timeout 240` sends SIGTERM at 240s (graceful), SIGKILL 5s
      # later if still running; the outer Python timeout gives us
      # 300s total to tolerate either.
      print("### smoke: nix copy -vvv --to ssh://root@cache <web01Closure>")
      rc, _ = builder.execute(
          "timeout --kill-after=5 240 "
          "/run/current-system/sw/bin/nix copy -vvv "
          "--to ssh://root@cache "
          "${web01Closure} > /tmp/nix-copy.log 2>&1; "
          "echo EXIT=$? >> /tmp/nix-copy.log",
          timeout=300,
      )
      print("=== nix-copy.log ===")
      print(builder.succeed("cat /tmp/nix-copy.log"))
      print("====================")
      copy_exit = builder.succeed(
          "grep '^EXIT=' /tmp/nix-copy.log | tail -1"
      ).strip()
      if copy_exit != "EXIT=0":
          raise Exception(
              f"nix copy smoke failed ({copy_exit}) — see log above"
          )
      print("### smoke tests passed")

      # --- Phase 2: R1 — nixfleet release create --push-to ssh://root@cache ---
      # The real cli::release::create runs; the shim returns canned
      # nix eval / nix build output; `nix copy` is delegated to real nix
      # and pushes the closure to the cache over SSH.
      print("### running nixfleet release create")
      rc, out = builder.execute(
          "bash -lc '"
          "export NIXFLEET_API_KEY=" + TEST_KEY + " && "
          "nixfleet "
          "--control-plane-url https://cp:8080 "
          "--ca-cert /etc/nixfleet-tls/ca.pem "
          "--client-cert /etc/nixfleet-tls/builder-cert.pem "
          "--client-key /etc/nixfleet-tls/builder-key.pem "
          "release create "
          "--flake /tmp/fake-flake "
          "--hosts web-01 "
          "--push-to ssh://root@cache 2>&1"
          "'",
          timeout=480,
      )
      if rc != 0:
          print("=== nixfleet release create FAILED ===")
          print(out)
          raise Exception(f"nixfleet release create returned {rc}")
      print("### nixfleet release create output:")
      print(out)

      # Positive: the CP now has exactly one release.
      releases = json.loads(
          builder.succeed(f"{CP_CURL_BUILDER} https://cp:8080/api/v1/releases")
      )
      assert len(releases) == 1, f"expected 1 release, got {len(releases)}"
      release_id = releases[0]["id"]
      assert release_id.startswith("rel-"), \
          f"expected release id with rel- prefix, got {release_id}"

      release_entries = releases[0]["entries"]
      assert len(release_entries) == 1, \
          f"expected 1 entry, got {len(release_entries)}"
      store_path = release_entries[0]["store_path"]
      assert "nixfleet-release-web-01" in store_path, \
          f"unexpected store path: {store_path}"

      # Positive: the closure is on the cache node after `nix copy`.
      cache.succeed(f"test -e {store_path}")

      # Negative: the closure was NOT pushed to the cp node.
      cp.fail(f"test -e {store_path}")

      # --- Phase 3: R2 — agent fetches from http://cache:5000 ---
      agent.wait_for_unit("multi-user.target")

      # Agent's nix config must list the cache as a substituter.
      agent.succeed(
          "grep -E '^substituters.*cache:5000' /etc/nix/nix.conf"
      )

      # Negative: the closure is NOT on the agent before the fetch.
      agent.fail(f"test -e {store_path}")

      # Fetch via substitution from the harmonia cache. The cache signs
      # paths on the fly using the pre-generated signing key, and the
      # agent's trusted-public-keys contains the matching public key
      # (set by services.nixfleet-cache).
      agent.succeed(
          f"nix copy --no-check-sigs --from http://cache:5000 {store_path}"
      )

      # Positive: the closure is now on the agent.
      agent.succeed(f"test -e {store_path}")
    '';
  }
