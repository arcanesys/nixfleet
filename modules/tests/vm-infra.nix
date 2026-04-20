# VM tests for infrastructure scopes (firewall, monitoring, backup, secrets, cache-server).
# Each test boots a single-node VM and asserts runtime state for one scope
# in isolation, without a control plane or a fleet topology.
#
# Most tests use `mkAgentNode` (with `dryRun = true` and no reachable CP)
# because that is the node shape every `_vm-fleet-scenarios/*.nix` already
# uses - identical args produce identical NixOS system closures, so Nix
# dedupes the base image across the fleet suite and these single-node
# tests. The agent service starts, fails its first poll against a
# non-existent `cp:8080`, schedules a retry, and stays active - it does
# not affect the subsystem assertions.
#
# `vm-cache-server` stays on raw `mkTestNode` because it needs a specific
# harmonia + LoadCredential shape that `mkAgentNode` does not model.
{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs inputs;};

    mkTestNode = helpers.mkTestNode {
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;

    # Single-node `mkAgentNode` with dryRun = true. Every subsystem test
    # calls this with its scope-specific `extraModules` and gets the
    # same base closure (agent service + mTLS wiring + shared certs).
    mkSubsystemNode = extraModules:
      helpers.mkAgentNode {
        inherit mkTestNode defaultTestSpec extraModules;
        testCerts = helpers.sharedTestCerts;
        hostName = "machine";
      };
  in
    # x86_64-linux only (VM tests require KVM)
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- vm-firewall: SSH rate limiting and drop logging ---
        vm-firewall = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-firewall";
          nodes.machine = mkSubsystemNode [];
          testScript = ''
            machine.wait_for_unit("multi-user.target")

            # nftables should be active
            machine.succeed("nft list ruleset | grep -q 'chain input'")

            # SSH rate limiting rules should be present
            machine.succeed("nft list ruleset | grep -q 'limit rate 5/minute'")

            # Drop logging should be enabled
            machine.succeed("nft list ruleset | grep -qi 'log'")
          '';
        };

        # --- vm-monitoring: node exporter responds on port ---
        vm-monitoring = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-monitoring";
          nodes.machine = mkSubsystemNode [
            {
              nixfleet.monitoring.nodeExporter = {
                enable = true;
                openFirewall = true;
              };
            }
          ];
          testScript = ''
            machine.wait_for_unit("multi-user.target")
            machine.wait_for_unit("prometheus-node-exporter.service")
            machine.wait_for_open_port(9100)

            # Verify metrics endpoint responds with Prometheus text format
            output = machine.succeed("curl -sf http://localhost:9100/metrics")
            assert "# HELP" in output, f"Expected Prometheus metrics, got: {output[:200]}"

            # Verify systemd collector is active (we enabled it)
            assert "node_systemd" in output, "Expected node_systemd metrics from systemd collector"
          '';
        };

        # --- vm-backup: timer registered and service skeleton exists ---
        vm-backup = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-backup";
          nodes.machine = mkSubsystemNode [
            ({pkgs, ...}: {
              nixfleet.backup = {
                enable = true;
                schedule = "*-*-* *:*:00";
                # Satisfy required-field assertions (the test overrides
                # ExecStart so restic never actually runs).
                restic.repository = "/tmp/dummy-repo";
                restic.passwordFile = "${pkgs.writeText "dummy-pw" "x"}";
              };
              # Provide a dummy ExecStart so the service can run
              systemd.services.nixfleet-backup.serviceConfig.ExecStart = "${pkgs.coreutils}/bin/true";
            })
          ];
          testScript = ''
            machine.wait_for_unit("multi-user.target")

            # Timer should be registered
            machine.succeed("systemctl list-timers | grep -q 'nixfleet-backup'")

            # Manually trigger the backup service
            machine.succeed("systemctl start nixfleet-backup.service")

            # Status file should be written after successful run
            machine.succeed("test -f /var/lib/nixfleet-backup/status.json")
            output = machine.succeed("cat /var/lib/nixfleet-backup/status.json")
            assert '"status": "success"' in output, f"Expected success status, got: {output}"
          '';
        };

        # --- vm-secrets: host key generation on first boot ---
        vm-secrets = pkgs.testers.runNixOSTest {
          node.specialArgs = {inherit inputs;};
          name = "vm-secrets";
          nodes.machine = mkSubsystemNode [
            {
              nixfleet.secrets.enable = true;
            }
          ];
          testScript = ''
            machine.wait_for_unit("multi-user.target")

            # Host key should exist (either pre-existing or generated by nixfleet-host-key-check)
            machine.succeed("test -f /etc/ssh/ssh_host_ed25519_key")
            machine.succeed("test -f /etc/ssh/ssh_host_ed25519_key.pub")

            # Key should have correct permissions
            machine.succeed("stat -c '%a' /etc/ssh/ssh_host_ed25519_key | grep -q '600'")
          '';
        };

        # --- vm-cache-server: harmonia binary cache server starts and responds ---
        #
        # Deliberately NOT using mkAgentNode - this test exercises a
        # specific harmonia + LoadCredential shape (signing key baked
        # into the store) that has no overlap with the fleet-agent
        # node shape. Forcing mkAgentNode here would bolt an unrelated
        # nixfleet-agent service onto the test without closure benefit.
        vm-cache-server = let
          # Generate a signing key at build time for the test.
          # `nix-store --generate-binary-cache-key` tries to mkdir
          # /nix/var/nix/profiles on startup, which is forbidden in
          # the build sandbox - redirect NIX_STATE_DIR into $TMPDIR
          # so it writes its profile scratch space there instead.
          # Also write the key into a subdirectory of $out so $out
          # remains a directory (what the nix build environment expects)
          # and the file is reachable via ${signingKeyFile}/signing.secret.
          signingKeyPair = pkgs.runCommand "cache-test-signing-key" {} ''
            mkdir -p $out
            export NIX_STATE_DIR="$TMPDIR/nix-state"
            mkdir -p "$NIX_STATE_DIR"
            ${pkgs.nix}/bin/nix-store --generate-binary-cache-key \
              test-cache \
              $out/signing.secret \
              $out/signing.public
            chmod 0444 $out/signing.secret
          '';
        in
          pkgs.testers.runNixOSTest {
            node.specialArgs = {inherit inputs;};
            name = "vm-cache-server";

            nodes.server = mkTestNode {
              hostSpecValues = defaultTestSpec // {hostName = "server";};
              extraModules = [
                {
                  services.nixfleet-cache-server = {
                    enable = true;
                    signingKeyFile = "${signingKeyPair}/signing.secret";
                  };
                }
              ];
            };

            testScript = ''
              server.wait_for_unit("multi-user.target")
              # services.nixfleet-cache-server is a thin wrapper around
              # services.harmonia.cache - the actual systemd unit is
              # `harmonia.service`, not `nixfleet-cache-server.service`.
              # Use a bounded wait_until_succeeds so a regression in
              # the signing-key LoadCredential path produces an
              # informative failure rather than an opaque
              # wait_for_unit hang.
              try:
                  server.wait_until_succeeds(
                      "systemctl is-active harmonia.service", timeout=60
                  )
              except Exception:
                  print("=== harmonia status ===")
                  print(server.execute("systemctl status harmonia.service --no-pager")[1])
                  print("=== harmonia journal ===")
                  print(server.execute("journalctl -u harmonia.service --no-pager -n 80")[1])
                  raise
              server.wait_for_open_port(5000)

              # Harmonia should respond with nix-cache-info
              server.succeed("curl -sf http://localhost:5000/nix-cache-info")
            '';
          };

        # --- vm-backup-restic: restic backend wires up correctly ---
        vm-backup-restic = let
          passwordFile = pkgs.writeText "restic-test-password" "test-password";
          repositoryPath = "/tmp/restic-test-repo";
        in
          pkgs.testers.runNixOSTest {
            node.specialArgs = {inherit inputs;};
            name = "vm-backup-restic";

            nodes.machine = mkSubsystemNode [
              {
                nixfleet.backup = {
                  enable = true;
                  backend = "restic";
                  schedule = "*-*-* *:*:00";
                  paths = ["/tmp"];
                  restic = {
                    repository = repositoryPath;
                    passwordFile = "${passwordFile}";
                  };
                };
              }
            ];

            testScript = ''
              machine.wait_for_unit("multi-user.target")

              # Backup timer should be registered
              machine.succeed("systemctl list-timers | grep nixfleet-backup")

              # restic binary should be available (installed by backend = "restic")
              machine.succeed("which restic")

              # Backup service ExecStart should reference restic
              machine.succeed("systemctl cat nixfleet-backup.service | grep restic")
            '';
          };
      };
    };
}
