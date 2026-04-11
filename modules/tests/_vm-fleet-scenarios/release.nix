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
      cp.start()
      cache.start()
      builder.start()
      agent.start()

      cp.wait_for_unit("nixfleet-control-plane.service")
      cp.wait_for_open_port(8080)
      cache.wait_for_unit("sshd.service")
      cache.wait_for_open_port(22)
      cache.wait_for_unit("harmonia.service")
      cache.wait_for_open_port(5000)

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

      # --- Phase 2: R1 — nixfleet release create --push-to ssh://root@cache ---
      # The real cli::release::create runs; the shim returns canned
      # nix eval / nix build output; `nix copy` is delegated to real nix
      # and pushes the closure to the cache over SSH.
      builder.succeed(
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
          "--push-to ssh://root@cache"
          "'"
      )

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
