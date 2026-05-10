# Smoke-path stub: logs signedAt only; verify lives in agent-verify.nix.
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

  # See agent-real.nix for rationale on these networking knobs.
  networking.firewall.enable = false;
  networking.useNetworkd = lib.mkDefault true;
  systemd.network.networks."10-vm-net" = {
    matchConfig.Name = "en* eth*";
    networkConfig.DHCP = "yes";
    # FOOTGUN: RequiredForOnline=routable; degraded fires before DHCP, masking failures.
    linkConfig.RequiredForOnline = "routable";
  };

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
      # journal+console routes the guest log to the host journal so
      # the scenario testScript can grep microvm@<agent>.service.
      StandardOutput = "journal+console";
      StandardError = "journal+console";
      ExecStartPre = "${pkgs.bash}/bin/bash -c 'for i in $(seq 1 60); do ${pkgs.iproute2}/bin/ip route show default | grep -q . && exit 0; sleep 1; done; echo \"harness-agent: no default route after 60s\" >&2; exit 1'";
      ExecStart = pkgs.writeShellScript "harness-agent-fetch" ''
        set -euo pipefail

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

        msg="harness-agent-ok: signedAt=$signed_at signatureAlgorithm=$algo"
        echo "$msg" >&2
        echo "$msg" > /dev/console || true
      '';
      Restart = "on-failure";
      RestartSec = 5;
    };
  };

  system.stateVersion = lib.mkDefault "24.11";
}
