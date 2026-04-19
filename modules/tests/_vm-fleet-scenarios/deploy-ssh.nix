# vm-fleet-deploy-ssh — D4
#
# Exercises the REAL `nixfleet deploy --hosts target --ssh --target root@target`
# orchestration path end-to-end WITHOUT any control plane:
#
#   1. `nix eval` to discover nixosConfigurations attribute names
#   2. `nix build` to produce the system closure
#   3. `nix-copy-closure --to root@target <path>`  (real transfer over SSH)
#   4. `ssh root@target <path>/bin/switch-to-configuration switch`
#
# There is intentionally NO `cp` node in this test — the whole point of D4 is
# that `--ssh` mode bypasses the control plane entirely. If the CLI ever
# reached out to a CP here, the test would fail because there is no CP
# reachable on the test network.
#
# Strategy: the shell `nix` shim (modules/tests/_lib/nix-shim.nix, same as
# R1/R2) intercepts `nix eval` and `nix build` on the operator node and
# returns canned answers pointing at a pre-built "stub toplevel" closure.
# `nix-copy-closure` and `ssh` are real — they actually transfer the stub
# closure to the target over the test network and invoke its
# `bin/switch-to-configuration switch`. The stub writes a marker file
# (`/tmp/stub-switch-called`) on the target so the test can prove the
# switch step actually ran end-to-end.
{
  pkgs,
  inputs,
  lib,
  mkTestNode,
  defaultTestSpec,
  ...
}: let
  # Certs are unused by deploy-ssh itself (no CP, no mTLS), but mkTestNode
  # does not care and we keep the helper symmetric with the other subtests.
  # A minimal "system toplevel" derivation with a real `bin/switch-to-configuration`
  # script. The script writes a marker file so the test can assert the switch
  # step actually ran on the target (proving nix-copy-closure + ssh-switch
  # completed the full chain), then exits 0 so the CLI sees success.
  stubToplevel = pkgs.runCommand "stub-toplevel" {} ''
    mkdir -p $out/bin
    cat > $out/bin/switch-to-configuration <<'EOF'
    #!/bin/sh
    echo "stub switch called: $*"
    mkdir -p /tmp
    printf 'switch-called %s\n' "$*" > /tmp/stub-switch-called
    exit 0
    EOF
    chmod +x $out/bin/switch-to-configuration
  '';

  # Pre-generated throwaway ed25519 SSH keypair (same material as release.nix).
  # Baked as literals to avoid IFD during nixosTest evaluation. This key has
  # no production value — it only authenticates the operator to the target
  # inside the test network.
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

  nixShim = import ../_lib/nix-shim.nix {inherit pkgs lib;} {
    hosts = [
      {
        name = "target";
        platform = "x86_64-linux";
        tags = [];
        storePath = "${stubToplevel}";
      }
    ];
  };

  nixfleetCli = pkgs.callPackage ../../../crates/cli {inherit inputs;};
in
  pkgs.testers.runNixOSTest {
    node.specialArgs = {inherit inputs;};
    name = "vm-fleet-deploy-ssh";

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
            pkgs.jq
            # Make the stub closure present on the operator store so
            # nix-copy-closure has something to transfer.
            stubToplevel
          ];

          # Prepend the shim's bin dir to PATH so `nix` resolves to the
          # shim. Do NOT add nixShim to systemPackages — that collides
          # with the real nix at /run/current-system/sw/bin/nix and, if
          # the shim wins the collision, its fallback branch causes an
          # infinite exec loop. The string interpolation below pulls
          # nixShim into the closure without installing it at the
          # colliding path.
          environment.sessionVariables.PATH =
            lib.mkBefore ["${nixShim}/bin"];

          # Private SSH key for ssh-ing into the target.
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

          # Pre-seed the operator's public key in root's authorized_keys so
          # nix-copy-closure and the subsequent ssh-switch can authenticate.
          users.users.root.openssh.authorizedKeys.keys = [testSshPublicKey];

          # Accept incoming unsigned store paths from the operator during
          # nix-copy-closure (we do not sign the stub closure).
          nix.settings = {
            trusted-users = ["root"];
            require-sigs = false;
          };

          networking.firewall.allowedTCPPorts = [22];
        }
      ];
    };

    testScript = let
      stubPath = "${stubToplevel}";
    in ''
      # --- Step 1: Start both nodes ---
      # There is NO cp node in this test topology: D4 proves that
      # `nixfleet deploy --ssh --target` never touches a control plane.
      target.start()
      operator.start()

      target.wait_for_unit("sshd.service")
      target.wait_for_open_port(22)
      operator.wait_for_unit("multi-user.target")

      # Sanity: the stub's switch marker must not exist before the deploy.
      # (We cannot usefully assert the stub STORE PATH is absent because
      # nixosTest mounts the host Nix store read-only on every VM over
      # 9p, so every /nix/store path referenced by any node is visible
      # everywhere. The real deploy proof is that switch-to-configuration
      # was invoked on the target, which we assert via the marker file.)
      target.fail("test -e /tmp/stub-switch-called")

      # --- Step 2: Prepare SSH client state on operator ---
      operator.succeed("mkdir -p /root/.ssh && chmod 700 /root/.ssh")
      operator.succeed("cp /etc/ssh-operator-key /root/.ssh/id_ed25519")
      operator.succeed("chmod 600 /root/.ssh/id_ed25519")
      # Pre-accept the target's host key so ssh / nix-copy-closure never prompt.
      operator.succeed("ssh-keyscan -t ed25519 target >> /root/.ssh/known_hosts")

      # The CLI is invoked with --flake /tmp/fake-flake. The CLI does not
      # validate the flake itself — only the nix-shim reads the reference
      # string — but we create a harmless placeholder just in case.
      operator.succeed(
          "mkdir -p /tmp/fake-flake && "
          "printf '{ outputs = _: {}; }\\n' > /tmp/fake-flake/flake.nix"
      )

      # Sanity: the shim must be first on PATH so `nix eval` and `nix build`
      # resolve to the canned responses instead of the real nix.
      which_nix = operator.succeed("bash -lc 'command -v nix'").strip()
      assert which_nix != "/run/current-system/sw/bin/nix", \
          f"shim not prepended to PATH, got {which_nix!r}"

      # --- Step 3: Run the real `nixfleet deploy --hosts target --ssh --target root@target` ---
      # The CLI will:
      #   1. `nix eval` attrNames  → shim returns '["target"]'
      #   2. `nix build ...toplevel --print-out-paths --no-link` → shim returns the stub path
      #   3. `nix-copy-closure --to root@target <stubPath>` → real transfer over SSH
      #   4. `ssh root@target <stubPath>/bin/switch-to-configuration switch` → writes marker
      deploy_out = operator.succeed(
          "bash -lc '"
          "nixfleet deploy "
          "--flake /tmp/fake-flake "
          "--hosts target "
          "--ssh "
          "--target root@target"
          "'"
      )
      assert "1 succeeded" in deploy_out, \
          f"expected '1 succeeded' in deploy output, got: {deploy_out!r}"

      # --- Step 4: Positive assertions on the target ---
      # Prove that `nix-copy-closure --to root@target` actually registered
      # the stub path in target's LOCAL Nix database. We cannot use
      # `test -e ${stubPath}` as a proof because nixosTest mounts the host
      # /nix/store read-only on every VM via 9p, so the store-path files
      # are visible on every node regardless of whether `nix-copy-closure`
      # ran. The Nix *database* (/nix/var/nix/db/db.sqlite) is VM-local,
      # though, so `nix path-info` is a load-bearing check: if the copy
      # step was skipped, the path would be visible as a file but missing
      # from the DB and `nix path-info` would fail.
      target.succeed("nix-store -q --references ${stubPath} >/dev/null")

      # The stub's `switch-to-configuration switch` was actually invoked
      # over SSH — it wrote the marker file. This proves the full
      # orchestration chain (discover → build → copy → switch) ran.
      marker = target.succeed("cat /tmp/stub-switch-called").strip()
      assert "switch-called" in marker, \
          f"expected switch-called marker, got: {marker!r}"
      assert "switch" in marker, \
          f"expected marker to contain 'switch' arg, got: {marker!r}"
    '';
  }
