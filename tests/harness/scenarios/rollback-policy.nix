# Drives Failed via direct DB injection because apply_actions only
# handles SoakHost; this scenario is about the wire round-trip, not the
# Failed inducement.
{
  harnessLib,
  testCerts,
  signedFixture,
  cpPkg,
  agentPkg,
  closureHash,
  agentName ? "agent-01",
  agentKeypairs,
  ...
}: let
  cpHostModule = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  sqliteHostModule = {pkgs, ...}: {
    environment.systemPackages = [pkgs.sqlite];
  };

  combinedHostModule = {
    imports = [cpHostModule sqliteHostModule];
  };

  preseedModule = harnessLib.convergencePreseedModule {inherit closureHash;};

  agentNode = harnessLib.mkRealAgentNode {
    inherit testCerts signedFixture agentPkg;
    hostName = agentName;
    pollIntervalSecs = 5;
    sshHostKey = "${agentKeypairs.${agentName}}/private.openssh";
    extraModules = [preseedModule];
  };

  agents = {${agentName} = agentNode;};
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-rollback-policy";
    cpHostModule = combinedHostModule;
    inherit agents;
    timeout = 600;
    testScript = ''
      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)
      host.wait_for_unit("microvms.target", timeout=300)
      host.wait_for_unit("microvm@${agentName}.service", timeout=300)

      print("step 1: waiting for initial agent checkin...")
      pre_inject_cursor = host.succeed("date '+%Y-%m-%d %H:%M:%S'").strip()
      wait_for_journal_match(
          host,
          since_cursor=pre_inject_cursor,
          unit="nixfleet-control-plane.service",
          pattern="checkin received.*${agentName}",
          timeout=180,
          label="initial agent checkin",
      )
      print("step 1: baseline checkin observed for ${agentName}")

      injected_rollout_id = "stable@injected-failure"
      print(f"step 2: injecting Failed state for ${agentName}@{injected_rollout_id}")
      # INSERT OR REPLACE: orphan-confirm recovery already UPSERT'd a
      # host_dispatch_state row for this host (PRIMARY KEY hostname).
      host.succeed(f"""sqlite3 /var/lib/nixfleet-cp/state.db <<'SQL'
      INSERT OR REPLACE INTO host_dispatch_state (
        hostname, rollout_id, channel, wave, target_closure_hash,
        target_channel_ref, state, dispatched_at, confirm_deadline
      ) VALUES (
        '${agentName}', '{injected_rollout_id}', 'stable', 0,
        '${closureHash}', '{injected_rollout_id}',
        'pending',
        datetime('now', '-30 seconds'),
        datetime('now', '+300 seconds')
      );
      SQL""")
      host.succeed(f"""sqlite3 /var/lib/nixfleet-cp/state.db <<'SQL'
      INSERT INTO dispatch_history (
        hostname, rollout_id, channel, wave, target_closure_hash,
        target_channel_ref, dispatched_at
      ) VALUES (
        '${agentName}', '{injected_rollout_id}', 'stable', 0,
        '${closureHash}', '{injected_rollout_id}',
        datetime('now', '-30 seconds')
      );
      SQL""")
      host.succeed(f"""sqlite3 /var/lib/nixfleet-cp/state.db <<'SQL'
      INSERT INTO host_rollout_state (
        rollout_id, hostname, host_state, updated_at
      ) VALUES (
        '{injected_rollout_id}', '${agentName}', 'Failed',
        datetime('now')
      );
      SQL""")

      pre_signal_cursor = host.succeed("date '+%Y-%m-%d %H:%M:%S'").strip()
      pre_state = host.succeed(f"""sqlite3 /var/lib/nixfleet-cp/state.db <<'SQL'
      SELECT host_state FROM host_rollout_state
      WHERE hostname='${agentName}' AND rollout_id='{injected_rollout_id}';
      SQL""").strip()
      assert pre_state == "Failed", f"expected Failed pre-signal, got {pre_state!r}"

      print("step 3: waiting for CP rollback-signal emission...")
      wait_for_journal_match(
          host,
          since_cursor=pre_signal_cursor,
          unit="nixfleet-control-plane.service",
          pattern="rollback-signal: emitting RollbackSignal",
          timeout=60,
          label="CP rollback-signal emission",
      )
      print("step 3: CP emitted rollback-signal as expected")

      print("step 4: waiting for agent-side rollback handling...")
      wait_for_journal_match(
          host,
          since_cursor=pre_signal_cursor,
          unit="microvm@${agentName}.service",
          pattern="CP issued rollback signal",
          timeout=60,
          label="agent rollback-signal handling",
      )
      print("step 4: agent-side rollback fired")

      print("step 5: waiting for Failed -> Reverted transition...")
      wait_for_journal_match(
          host,
          since_cursor=pre_signal_cursor,
          unit="nixfleet-control-plane.service",
          pattern="RollbackTriggered: host_rollout_state Failed . Reverted",
          timeout=60,
          label="Failed → Reverted transition",
      )
      print("step 5: Failed -> Reverted transition observed")

      print("step 6: asserting terminal stamps on host_dispatch_state + dispatch_history...")
      stamp_deadline = time.monotonic() + 15
      op_state = ""
      audit_terminal = ""
      # FOOTGUN: nixosTest dedents testScript to column 0; heredocs nested
      # in this while loop sit at column 4 after dedent and bash treats
      # the EOF marker as content. Use inline SQL strings, not heredocs.
      op_q = (
          f"SELECT state FROM host_dispatch_state "
          f"WHERE hostname='${agentName}' "
          f"AND rollout_id='{injected_rollout_id}';"
      )
      audit_q = (
          f"SELECT IFNULL(terminal_state, 'NULL') FROM dispatch_history "
          f"WHERE hostname='${agentName}' "
          f"AND rollout_id='{injected_rollout_id}';"
      )
      while time.monotonic() < stamp_deadline:
          op_state = host.succeed(
              f'sqlite3 /var/lib/nixfleet-cp/state.db "{op_q}"'
          ).strip()
          audit_terminal = host.succeed(
              f'sqlite3 /var/lib/nixfleet-cp/state.db "{audit_q}"'
          ).strip()
          if op_state == "rolled-back" and audit_terminal == "rolled-back":
              break
          time.sleep(2)
      assert op_state == "rolled-back", (
          f"host_dispatch_state.state did not flip to 'rolled-back' "
          f"for {injected_rollout_id}; got {op_state!r}"
      )
      assert audit_terminal == "rolled-back", (
          f"dispatch_history.terminal_state did not stamp 'rolled-back' "
          f"for {injected_rollout_id}; got {audit_terminal!r}"
      )
      print("step 6: terminal stamps observed on both tables")

      print("step 7: waiting for two more polls + asserting no re-emission...")
      # 2s sleep so journalctl --since (rounded down) excludes the
      # original pre-Reverted emission.
      host.succeed("sleep 2")
      post_revert_cursor = host.succeed("date '+%Y-%m-%d %H:%M:%S'").strip()
      time.sleep(15)
      rc, _ = host.execute(
          "journalctl -u nixfleet-control-plane.service "
          f"--since='{post_revert_cursor}' --no-pager "
          "| grep -E 'rollback-signal: emitting RollbackSignal'"
      )
      if rc == 0:
          cp_dump = host.succeed(
              "journalctl -u nixfleet-control-plane.service "
              f"--since='{post_revert_cursor}' --no-pager"
          )
          print("=== CP journal (no rollback-signal expected) ===")
          print(cp_dump)
          print("=== end ===")
          raise Exception(
              "CP re-emitted rollback-signal after Reverted transition"
          )

      print(
          "fleet-harness-rollback-policy: rollback-and-halt round-trip "
          "holds - Failed → CP RollbackSignal → agent rollback → "
          "RollbackTriggered → Reverted → emission stops."
      )
    '';
  }
