{
  lib,
  pkgs,
  testCerts,
  controlPlaneHost,
  controlPlanePort,
  harnessMicrovmDefaults,
  agentHostName,
  signedFixture,
  verifyArtifactPkg,
  now,
  freshnessWindowSecs,
  ...
}: {
  microvm = harnessMicrovmDefaults;

  # See agent-real.nix for rationale on these networking knobs.
  networking.firewall.enable = false;
  networking.useNetworkd = lib.mkDefault true;
  systemd.network.networks."10-vm-net" = {
    matchConfig.Name = "en* eth*";
    networkConfig.DHCP = "yes";
    # FOOTGUN: RequiredForOnline=routable; default "degraded" fires before DHCP, masking failures.
    linkConfig.RequiredForOnline = "routable";
  };

  environment.etc = {
    "nixfleet-harness/ca.pem".source = "${testCerts}/ca.pem";
    "nixfleet-harness/${agentHostName}-cert.pem".source = "${testCerts}/${agentHostName}-cert.pem";
    "nixfleet-harness/${agentHostName}-key.pem".source = "${testCerts}/${agentHostName}-key.pem";
    "nixfleet-harness/test-trust.json".source = "${signedFixture}/test-trust.json";
  };

  systemd.services.harness-agent = {
    description = "Nixfleet harness agent (verifies signed artifact)";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    path = [pkgs.curl pkgs.coreutils verifyArtifactPkg];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      StandardOutput = "journal+console";
      StandardError = "journal+console";
      ExecStartPre = "${pkgs.bash}/bin/bash -c 'for i in $(seq 1 60); do ${pkgs.iproute2}/bin/ip route show default | grep -q . && exit 0; sleep 1; done; echo \"harness-agent: no default route after 60s\" >&2; exit 1'";
      ExecStart = pkgs.writeShellScript "harness-agent-verify" ''
        set -euo pipefail

        base="https://cp:${toString controlPlanePort}"
        workdir=$(mktemp -d)
        trap 'rm -rf "$workdir"' EXIT

        fetch() {
          local url_path="$1" out="$2"
          curl -sfS \
            --cacert /etc/nixfleet-harness/ca.pem \
            --cert /etc/nixfleet-harness/${agentHostName}-cert.pem \
            --key /etc/nixfleet-harness/${agentHostName}-key.pem \
            --resolve "cp:${toString controlPlanePort}:${controlPlaneHost}" \
            --connect-timeout 30 \
            --max-time 60 \
            "$base$url_path" -o "$out"
        }

        # Microvm boot races CP startup; budget 60s before treating as outage.
        echo "harness-agent: waiting for CP to accept TLS" >&2
        for attempt in $(seq 1 30); do
          if curl -sfS \
            --cacert /etc/nixfleet-harness/ca.pem \
            --cert /etc/nixfleet-harness/${agentHostName}-cert.pem \
            --key /etc/nixfleet-harness/${agentHostName}-key.pem \
            --resolve "cp:${toString controlPlanePort}:${controlPlaneHost}" \
            --connect-timeout 2 --max-time 4 \
            -o /dev/null "$base/canonical.json" 2>/dev/null; then
            break
          fi
          if [ "$attempt" -eq 30 ]; then
            echo "harness-roundtrip-FAIL: CP unreachable after 60s" >&2
            exit 1
          fi
          sleep 2
        done

        echo "harness-agent: fetching signed artifact from $base" >&2
        if ! fetch /canonical.json "$workdir/artifact"; then
          echo "harness-roundtrip-FAIL: canonical.json fetch failed" >&2
          exit 1
        fi
        if ! fetch /canonical.json.sig "$workdir/signature"; then
          echo "harness-roundtrip-FAIL: canonical.json.sig fetch failed" >&2
          exit 1
        fi

        sig_len=$(stat -c %s "$workdir/signature")
        if [ "$sig_len" != 64 ]; then
          echo "harness-roundtrip-FAIL: expected 64-byte signature, got $sig_len" >&2
          exit 1
        fi

        echo "harness-agent: running nixfleet-verify-artifact" >&2
        verify_out=$(nixfleet-verify-artifact artifact \
          --artifact "$workdir/artifact" \
          --signature "$workdir/signature" \
          --trust-file /etc/nixfleet-harness/test-trust.json \
          --now ${now} \
          --freshness-window-secs ${toString freshnessWindowSecs})

        msg="harness-roundtrip-ok: $verify_out"
        echo "$msg" >&2
        echo "$msg" > /dev/console || true
      '';
      Restart = "on-failure";
      RestartSec = 5;
    };
  };

  system.stateVersion = lib.mkDefault "24.11";
}
