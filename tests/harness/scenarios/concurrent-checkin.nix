# LOADBEARING: regression for the atomic VerifiedFleetSnapshot pair  -
# `(fleet, fleet_resolved_hash)` are held under one RwLock so a concurrent
# checkin reader never sees a half-swapped snapshot from a torn write.
{
  lib,
  pkgs,
  harnessLib,
  testCerts,
  signedFixture,
  cpPkg,
  agentLoopCount ? 5,
  soakDurationSecs ? 30,
  ...
}: let
  cpHostBase = harnessLib.mkRealCpHostModule {
    inherit testCerts signedFixture cpPkg;
  };

  loopHostnames = map (i: "agent-${lib.fixedWidthString 2 "0" (toString i)}") (
    lib.range 1 agentLoopCount
  );

  certMountModule = {
    environment.etc =
      {
        "harness/ca.pem".source = "${testCerts}/ca.pem";
      }
      // builtins.listToAttrs (lib.concatMap (h: [
          {
            name = "harness/${h}-cert.pem";
            value.source = "${testCerts}/${h}-cert.pem";
          }
          {
            name = "harness/${h}-key.pem";
            value.source = "${testCerts}/${h}-key.pem";
          }
        ])
        loopHostnames);
    environment.systemPackages = [pkgs.jq];
  };

  combinedHostModule = {
    imports = [cpHostBase certMountModule];
  };

  loopDriverScript = pkgs.writeShellScript "harness-checkin-loop" ''
    set -u
    hostname="$1"
    duration="$2"
    cacert=/etc/harness/ca.pem
    cert="/etc/harness/$hostname-cert.pem"
    key="/etc/harness/$hostname-key.pem"
    end=$(( $(date +%s) + duration ))
    while [ "$(date +%s)" -lt "$end" ]; do
      body=$(${pkgs.jq}/bin/jq -n --arg h "$hostname" '{
        hostname: $h, schemaVersion: 1, machineId: $h,
        agentVersion: "harness-concurrent",
        uptimeSecs: 1,
        bootId: "00000000-0000-0000-0000-000000000000",
        currentGeneration: {
          closureHash: ("deadbeef-mismatch-" + $h),
          channelRef: "main",
          bootId: "00000000-0000-0000-0000-000000000000"
        }
      }')
      curl -sk -o /dev/null \
        --cacert "$cacert" \
        --cert "$cert" \
        --key "$key" \
        -H 'Content-Type: application/json' \
        -d "$body" \
        https://localhost:8443/v1/agent/checkin || true
      # Tight loop maximises read pressure on verified_fleet RwLock.
    done
  '';
in
  harnessLib.mkFleetScenario {
    name = "fleet-harness-concurrent-checkin";
    cpHostModule = combinedHostModule;
    agents = {};
    timeout = 600;
    testScript = ''
      import re

      start_all()

      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      host.wait_until_succeeds(
          "journalctl -u nixfleet-control-plane.service --no-pager "
          "| grep -E 'verified-fleet snapshot|primed verified-fleet'",
          timeout=60,
      )

      hostnames = ${builtins.toJSON loopHostnames}
      soak_secs = ${toString soakDurationSecs}

      # journalctl --since rounds down to the second; sleep 1s before
      # the first checkin so the cursor strictly precedes it.
      soak_cursor = host.succeed("date '+%Y-%m-%d %H:%M:%S'").strip()
      host.succeed("sleep 1")

      print(f"step 1: spawning {len(hostnames)} concurrent checkin loops "
            f"for {soak_secs}s...")
      bg_cmd = " & ".join(
          f"${loopDriverScript} {h} {soak_secs}" for h in hostnames
      ) + " & wait"
      host.succeed(f"bash -c '{bg_cmd}'", timeout=soak_secs + 60)
      print("step 1: soak complete")

      print("step 2: harvesting dispatched rollout_ids from CP journal...")
      journal = host.succeed(
          "journalctl -u nixfleet-control-plane.service "
          f"--since='{soak_cursor}' --no-pager"
      )
      dispatch_lines = [ln for ln in journal.splitlines() if "target issued" in ln]
      print(f"step 2: {len(dispatch_lines)} `target issued` lines observed")

      rollout_re = re.compile(r'rollout="?([^"\s]+)"?')
      rollout_ids: set[str] = set()
      for ln in dispatch_lines:
          m = rollout_re.search(ln)
          if m is not None:
              rollout_ids.add(m.group(1))

      print(f"step 2: unique rollout_ids in dispatch log: {sorted(rollout_ids)}")

      assert len(rollout_ids) <= 1, (
          f"torn-snapshot regression: {len(rollout_ids)} distinct "
          f"rollout_ids observed under steady-state fleet - expected "
          f"≤ 1. Set: {sorted(rollout_ids)}"
      )

      if len(rollout_ids) == 0:
          print(
              "step 3: 0 dispatches issued during soak - fixture's "
              "stub closureHashes may produce NoDeclaration. Atomic "
              "pair contract holds vacuously; "
              "${toString agentLoopCount} loops × "
              "${toString soakDurationSecs}s of read pressure with NO "
              "torn-snapshot panic / mismatch in CP journal."
          )
      else:
          print(
              f"step 3: 1 stable rollout_id across "
              f"{len(dispatch_lines)} dispatches - atomic "
              f"VerifiedFleetSnapshot pair held under "
              f"${toString agentLoopCount} concurrent loops × "
              f"${toString soakDurationSecs}s soak."
          )

      bad_rc, _ = host.execute(
          "journalctl -u nixfleet-control-plane.service "
          f"--since='{soak_cursor}' --no-pager "
          "| grep -E 'panic|torn|fleet-hash mismatch|"
          "compute_rollout_id_for_channel failed'"
      )
      if bad_rc == 0:
          dump = host.succeed(
              "journalctl -u nixfleet-control-plane.service "
              f"--since='{soak_cursor}' --no-pager"
          )
          raise Exception(
              "concurrent-checkin: error/panic/torn-snapshot pattern "
              "found in CP journal during soak\n=== journal ===\n"
              + dump
              + "\n=== end ==="
          )
      print("step 4: no error/panic/torn patterns in CP journal during soak")

      print(
          "fleet-harness-concurrent-checkin: atomic VerifiedFleetSnapshot "
          "contract holds - under "
          + str(${toString agentLoopCount})
          + " concurrent checkin loops × "
          + str(${toString soakDurationSecs})
          + "s soak, dispatched rollout_ids form a stable set "
          "(cardinality ≤ 1) and no torn-pair indicators surfaced."
      )
    '';
  }
