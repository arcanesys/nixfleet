{
  harnessLib,
  testCerts,
  signedFixture,
  cpPkg,
  ...
}: let
  cpHostModule = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  # Short confirm-deadline so step 1's manually-injected row expires
  # within the test's first action. Reuses the module-managed ExecStart
  # (which reads artifact/signature/trust from signedFixture's nix-store
  # path) rather than a hand-rolled ExecStart referencing
  # /etc/nixfleet-cp/* paths that no longer get staged.
  shortDeadlineModule = {
    services.nixfleet-control-plane.confirmDeadlineSecs = 3;
    environment.etc = {
      "harness/agent-cert.pem".source = "${testCerts}/agent-01-cert.pem";
      "harness/agent-key.pem".source = "${testCerts}/agent-01-key.pem";
      "harness/ca.pem".source = "${testCerts}/ca.pem";
    };
  };

  sqliteHostModule = {pkgs, ...}: {
    environment.systemPackages = [pkgs.sqlite];
  };

  combinedHostModule = {
    imports = [cpHostModule shortDeadlineModule sqliteHostModule];
  };
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-deadline-expiry";
    cpHostModule = combinedHostModule;
    agents = {};
    timeout = 300;
    testScript = ''
      import json

      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      print("step 1: inject expired host_dispatch_state row…")
      host.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db \""
          "INSERT INTO host_dispatch_state ("
          "  hostname, rollout_id, channel, wave,"
          "  target_closure_hash, target_channel_ref,"
          "  state, dispatched_at, confirm_deadline"
          ") VALUES ("
          "  'agent-01', 'stable@expired1', 'stable', 0,"
          "  'deadbeef-stub-closure', 'main',"
          "  'pending',"
          "  datetime('now', '-60 seconds'),"
          "  datetime('now', '-30 seconds')"
          ");"
          "\""
      )

      pre_state = host.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db "
          "\"SELECT state FROM host_dispatch_state WHERE rollout_id='stable@expired1';\""
      ).strip()
      assert pre_state == "pending", f"expected pending pre-confirm, got {pre_state!r}"

      print("step 2: POST /v1/agent/confirm against expired row…")
      confirm_body = {
          "hostname": "agent-01",
          "rollout": "stable@expired1",
          "wave": 0,
          "generation": {
              "closureHash": "deadbeef-stub-closure",
              "channelRef": "main",
              "bootId": "00000000-0000-0000-0000-000000000000",
          },
      }

      rc, out = host.execute(
          "curl -sk -o /dev/null -w '%{http_code}' "
          "--cacert /etc/harness/ca.pem "
          "--cert /etc/harness/agent-cert.pem "
          "--key /etc/harness/agent-key.pem "
          "-H 'Content-Type: application/json' "
          f"-d '{json.dumps(confirm_body)}' "
          "https://localhost:8443/v1/agent/confirm"
      )
      assert rc == 0, f"curl failed: {out}"
      assert out.strip() == "410", (
          f"expected HTTP 410 for expired-deadline confirm, got {out.strip()!r}"
      )
      print("step 2: 410 received as expected")

      print("step 3: assert row marked rolled-back…")
      post_state = host.succeed(
          "sqlite3 /var/lib/nixfleet-cp/state.db "
          "\"SELECT state FROM host_dispatch_state WHERE rollout_id='stable@expired1';\""
      ).strip()
      assert post_state == "rolled-back", (
          f"expected rolled-back state after 410, got {post_state!r}"
      )

      print(
          "fleet-harness-deadline-expiry: deadline-expiry contract holds — "
          "expired host_dispatch_state returns 410, row transitions to rolled-back."
      )
    '';
  }
