# tests/harness/nodes/agent-verify.nix
#
# Signed-roundtrip agent microVM. At boot, fetches `canonical.json` and
# `canonical.json.sig` from the CP over mTLS, stages `test-trust.json`
# from the signed fixture, then runs `nixfleet-verify-artifact`. On
# successful verify the unit emits `harness-roundtrip-ok:
# schemaVersion=<n> hosts=<n>` — the scenario testScript greps for the
# marker.
#
# TODO: retire this module when the v0.2 agent inlines the verify call
# site (per docs/phase-2-entry-spec.md §6 — the CLI is scaffold).
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
  # Fixed timestamps chosen so the freshness-window check always passes
  # against the fixture's frozen `signedAt = 2026-05-01T00:00:00Z`:
  #   now − signedAt = 3600s (1h) << freshnessWindow = 604800s (7d).
  # See tests/harness/fixtures/signed/default.nix for the stamp source.
  # Defaults live in mkVerifyingAgentNode, not here: NixOS's module
  # system resolves function arguments through `_module.args` and does
  # not consult module-function defaults.
  now,
  freshnessWindowSecs,
  ...
}: {
  microvm = harnessMicrovmDefaults;

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
        verify_out=$(nixfleet-verify-artifact \
          --artifact "$workdir/artifact" \
          --signature "$workdir/signature" \
          --trust-file /etc/nixfleet-harness/test-trust.json \
          --now ${now} \
          --freshness-window-secs ${toString freshnessWindowSecs})

        # Belt-and-suspenders: also write to /dev/console so the marker
        # reaches the host journal even if journald forwarding from the
        # guest is disabled (same pattern as nodes/agent.nix).
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
