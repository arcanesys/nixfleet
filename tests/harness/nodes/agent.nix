# tests/harness/nodes/agent.nix
#
# Agent microVM node for the harness.
#
# Stub behavior: at boot, curl the CP's /fleet.resolved.json over mTLS and
# log meta.signedAt via systemd. A successful fetch is recorded as a
# `harness-agent-ok` journal line that the scenario testScript greps for.
#
# TODO(5): once the v0.2 agent skeleton lands, replace the curl+jq unit
# with `services.nixfleet-agent` pointing at the harness CP. The mTLS
# wiring, controlPlaneHost/Port args, and the "successful fetch" signal
# stay the same — only the binary changes.
#
# TODO(5): Stream C's p256 signature verify path plugs in right here:
# once `meta.signatureAlgorithm` is populated, the agent must refuse to
# apply when the signature does not verify. For now we only log signedAt;
# the assertion is a placeholder that always passes if the JSON parses.
{
  lib,
  pkgs,
  testCerts,
  controlPlaneHost,
  controlPlanePort,
  harnessMicrovmDefaults,
  agentHostName,
  ...
}: {
  microvm = harnessMicrovmDefaults;

  environment.etc = {
    "nixfleet-harness/ca.pem".source = "${testCerts}/ca.pem";
    "nixfleet-harness/${agentHostName}-cert.pem".source = "${testCerts}/${agentHostName}-cert.pem";
    "nixfleet-harness/${agentHostName}-key.pem".source = "${testCerts}/${agentHostName}-key.pem";
  };

  systemd.services.harness-agent = {
    description = "Nixfleet harness agent stub (fetches fleet.resolved.json)";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    path = [pkgs.curl pkgs.jq pkgs.coreutils];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      # The harness scenario greps the host-VM journal for
      # microvm@<agent>.service, which only surfaces lines that reach
      # the guest's /dev/console. systemd units log to journald by
      # default, so without explicit forwarding the guest's journal
      # stays invisible from the host. journal+console routes both.
      StandardOutput = "journal+console";
      StandardError = "journal+console";
      # The `harness-agent-ok` marker is what the scenario greps on.
      # Emit it only when both the curl and the jq parse succeed.
      ExecStart = pkgs.writeShellScript "harness-agent-fetch" ''
        set -euo pipefail

        # URL uses the hostname `cp` so curl's SNI + cert check matches
        # the CP's server cert (CN=cp, issued by mkTlsCerts). --resolve
        # maps that hostname to the qemu user-net gateway IP, which
        # from inside a microVM is the host VM (where the CP stub runs).
        url="https://cp:${toString controlPlanePort}/"
        resp=$(mktemp)
        trap 'rm -f "$resp"' EXIT

        echo "harness-agent: fetching $url (via ${controlPlaneHost})" >&2
        if ! curl -sfS \
          --cacert /etc/nixfleet-harness/ca.pem \
          --cert /etc/nixfleet-harness/${agentHostName}-cert.pem \
          --key /etc/nixfleet-harness/${agentHostName}-key.pem \
          --resolve "cp:${toString controlPlanePort}:${controlPlaneHost}" \
          --connect-timeout 30 \
          --max-time 60 \
          "$url" > "$resp" 2>&1; then
          echo "harness-agent-FAIL: curl exited non-zero" >&2
          exit 1
        fi

        signed_at=$(jq -r '.meta.signedAt // "null"' < "$resp")
        algo=$(jq -r '.meta.signatureAlgorithm // "null"' < "$resp")

        # Belt-and-suspenders: also write directly to /dev/console so
        # the marker reaches the host journal even if journald forwarding
        # is ever disabled in the guest.
        msg="harness-agent-ok: signedAt=$signed_at signatureAlgorithm=$algo"
        echo "$msg" >&2
        echo "$msg" > /dev/console || true

        # TODO(5): invoke Stream C's p256 verify step here once the meta
        # signature block is populated. For the scaffold we just log.
      '';
      Restart = "on-failure";
      RestartSec = 5;
    };
  };

  system.stateVersion = lib.mkDefault "24.11";
}
