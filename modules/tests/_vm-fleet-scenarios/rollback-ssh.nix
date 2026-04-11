# vm-fleet-rollback-ssh — RB2
#
# Exercises the REAL `nixfleet rollback --host target --ssh --generation <path>`
# orchestration path end-to-end WITHOUT any control plane:
#
#   1. Deploy a "G2" stub closure via `nixfleet deploy --hosts target --ssh
#      --target root@target`. The real CLI path goes through the nix-shim
#      (intercepts `nix eval` / `nix build`) and then `nix-copy-closure` +
#      `ssh root@target <G2>/bin/switch-to-configuration switch`.
#
#   2. Roll back via `nixfleet rollback --host target --ssh --generation
#      <G1>`. The real rollback handler in `cli/src/main.rs::rollback` SSHes
#      to the target and runs `<G1>/bin/switch-to-configuration switch` with
#      the caller-supplied generation path (bypassing the
#      /nix/var/nix/profiles/system-1-link lookup — which is what the
#      `--generation` flag is for).
#
# Each stub closure writes a distinct marker to /tmp/stub-switch-last so the
# test can prove which generation is currently "active" after each phase.
#
# As with deploy-ssh.nix, there is intentionally NO cp node in this test —
# RB2 is about proving the SSH-mode rollback path works without any control
# plane reachable.
{
  pkgs,
  lib,
  mkTestNode,
  defaultTestSpec,
  ...
}: let
  # mkTlsCerts is not strictly needed (no mTLS here) but we accept it to
  # keep the subtest signature symmetric with the other scenarios.
  # Build two stub closures. Each has a real `bin/switch-to-configuration`
  # that writes a distinct marker file so the testScript can observe which
  # generation was most recently applied.
  mkStubClosure = label:
    pkgs.runCommand "stub-toplevel-${label}" {} ''
      mkdir -p $out/bin
      cat > $out/bin/switch-to-configuration <<EOF
      #!/bin/sh
      mkdir -p /tmp
      printf 'active=${label} args=%s\n' "\$*" > /tmp/stub-switch-last
      exit 0
      EOF
      chmod +x $out/bin/switch-to-configuration
    '';

  stubG1 = mkStubClosure "g1";
  stubG2 = mkStubClosure "g2";

  # Pre-generated throwaway ed25519 SSH keypair, same material as
  # deploy-ssh.nix / release.nix. Test-only.
  testSshPublicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILjq8UWKdMurTHKPfL8+vESysUAR5gaBYH5X/QrSVp3a nixfleet-test-operator";

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

  testSshPrivateKeyFile = pkgs.writeText "nixfleet-test-operator-key" testSshPrivateKey;

  # Shim intercepts `nix eval` / `nix build` on the operator so
  # `nixfleet deploy` returns stubG2 as the target's toplevel.
  nixShim = import ../_lib/nix-shim.nix {inherit pkgs lib;} {
    hosts = [
      {
        name = "target";
        platform = "x86_64-linux";
        tags = [];
        storePath = "${stubG2}";
      }
    ];
  };

  nixfleetCli = pkgs.callPackage ../../../cli {};
in
  pkgs.testers.nixosTest {
    name = "vm-fleet-rollback-ssh";

    nodes.operator = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "operator";
        };
      extraModules = [
        {
          environment.systemPackages = [
            nixfleetCli
            pkgs.openssh
            nixShim
            # Both stubs must be in the operator's store so nix-copy-closure
            # can transfer G2 (deploy path) and the rollback CLI can
            # reference G1's path as an existing store path.
            stubG1
            stubG2
          ];

          environment.sessionVariables.PATH =
            lib.mkBefore ["${nixShim}/bin"];

          environment.etc."ssh-operator-key" = {
            source = testSshPrivateKeyFile;
            mode = "0400";
          };
        }
      ];
    };

    nodes.target = mkTestNode {
      hostSpecValues =
        defaultTestSpec
        // {
          hostName = "target";
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

          users.users.root.openssh.authorizedKeys.keys = [testSshPublicKey];

          # Accept unsigned store paths from the operator during
          # nix-copy-closure.
          nix.settings = {
            trusted-users = ["root"];
            require-sigs = false;
          };

          networking.firewall.allowedTCPPorts = [22];
        }
      ];
    };

    testScript = let
      stubG1Path = "${stubG1}";
      stubG2Path = "${stubG2}";
    in ''
      # --- Phase 1: Start both nodes; no CP in this topology ---
      target.start()
      operator.start()

      target.wait_for_unit("sshd.service")
      target.wait_for_open_port(22)
      operator.wait_for_unit("multi-user.target")

      # Sanity: the marker file does not exist yet on the target.
      #
      # We cannot assert the stub STORE PATHS are absent because
      # nixosTest mounts the host /nix/store read-only on every VM
      # via 9p, so any path referenced anywhere in the test eval is
      # visible on every node regardless of its closure. The Nix
      # DATABASE is per-VM, though, so we can (and do below) use
      # `nix-store -q --references` as a load-bearing check for
      # "the path was actually copied into this VM's store DB".
      target.fail("test -e /tmp/stub-switch-last")

      # --- Phase 2: Prepare SSH client state on operator ---
      operator.succeed("mkdir -p /root/.ssh && chmod 700 /root/.ssh")
      operator.succeed("cp /etc/ssh-operator-key /root/.ssh/id_ed25519")
      operator.succeed("chmod 600 /root/.ssh/id_ed25519")
      operator.succeed("ssh-keyscan -t ed25519 target >> /root/.ssh/known_hosts")

      operator.succeed(
          "mkdir -p /tmp/fake-flake && "
          "printf '{ outputs = _: {}; }\\n' > /tmp/fake-flake/flake.nix"
      )

      which_nix = operator.succeed("bash -lc 'command -v nix'").strip()
      assert which_nix != "/run/current-system/sw/bin/nix", \
          f"shim not prepended to PATH, got {which_nix!r}"

      # --- Phase 3: Deploy G2 via `nixfleet deploy --hosts target --ssh` ---
      # Shim returns stubG2 from `nix build`. nix-copy-closure transfers it;
      # ssh runs its switch-to-configuration; the stub writes "active=g2".
      operator.succeed(
          "bash -lc '"
          "nixfleet deploy "
          "--flake /tmp/fake-flake "
          "--hosts target "
          "--ssh "
          "--target root@target"
          "'"
      )

      # Positive: G2 is registered in the target's Nix DB — the VM-local
      # `nix-store -q --references` only succeeds for paths the node's
      # database actually knows about (i.e. those added via
      # nix-copy-closure / substitution / local build), not for paths
      # merely visible via the 9p store overlay. Marker file also
      # confirms switch-to-configuration was invoked with G2.
      target.succeed("nix-store -q --references ${stubG2Path} >/dev/null")
      marker_after_deploy = target.succeed("cat /tmp/stub-switch-last").strip()
      assert "active=g2" in marker_after_deploy, \
          f"expected marker active=g2 after deploy, got: {marker_after_deploy!r}"

      # Sanity: G1 is NOT yet in the target's Nix DB. It will be after
      # the explicit nix-copy-closure in Phase 4 below. Using the DB
      # check (not `test -e`) because the store path is visible on the
      # target's filesystem via 9p even when the DB has no record of it.
      target.fail("nix-store -q --references ${stubG1Path}")

      # --- Phase 4: Pre-copy G1 to target so --generation can reference it ---
      # The rollback handler SSHes to the target and runs
      #   <generation>/bin/switch-to-configuration switch
      # directly; it does NOT nix-copy-closure the rollback path. So G1 must
      # already exist on the target's store. Operators in the real world use
      # --generation only for paths already present (e.g. previous profile
      # via /nix/var/nix/profiles/system-1-link). We mirror that precondition
      # by explicitly nix-copy-closure'ing G1 first.
      operator.succeed(
          "nix-copy-closure --to root@target ${stubG1Path}"
      )
      # Load-bearing: G1 is now in target's Nix DB.
      target.succeed("nix-store -q --references ${stubG1Path} >/dev/null")

      # Sanity: the marker is still G2 (the pre-copy is a transport-only
      # step, it does not invoke switch-to-configuration).
      marker_still_g2 = target.succeed("cat /tmp/stub-switch-last").strip()
      assert "active=g2" in marker_still_g2, \
          f"marker unexpectedly changed to {marker_still_g2!r} after nix-copy-closure"

      # --- Phase 5: `nixfleet rollback --host target --ssh --generation <G1>` ---
      operator.succeed(
          f"bash -lc '"
          f"nixfleet rollback "
          f"--host target "
          f"--ssh "
          f"--generation ${stubG1Path}"
          f"'"
      )

      # --- Phase 6: Positive assertion on the target ---
      marker_after_rollback = target.succeed("cat /tmp/stub-switch-last").strip()
      assert "active=g1" in marker_after_rollback, \
          f"expected marker active=g1 after rollback, got: {marker_after_rollback!r}"

      # Positive: the G2 stub is still registered in the target's Nix DB
      # (rollback did not delete the forward generation — history
      # preserved). And the marker does NOT still say g2.
      target.succeed("nix-store -q --references ${stubG2Path} >/dev/null")
      assert "active=g2" not in marker_after_rollback, \
          f"marker should have been overwritten by g1, still contains g2: {marker_after_rollback!r}"
    '';
  }
