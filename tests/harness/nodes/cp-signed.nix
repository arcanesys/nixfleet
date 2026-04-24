# tests/harness/nodes/cp-signed.nix
#
# Signed-roundtrip CP node. Serves two files from the Phase 2 signed
# fixture over mTLS on :8443, routed by request path:
#
#   GET /canonical.json      -> application/json
#   GET /canonical.json.sig  -> application/octet-stream (raw 64 bytes)
#
# Unlike the unsigned stub in `cp.nix`, this responder streams the file
# body with `cat` instead of slurping through `$(...)` — the signature
# is binary and contains null bytes, which shell command substitution
# mangles on every implementation except bash 5+.
#
# TODO: retire this module when `services.nixfleet-control-plane`
# gains the artifact-serve endpoint. The wire shape (two paths, mTLS,
# path-routed) is the real CP's contract.
{
  lib,
  pkgs,
  testCerts,
  signedFixture,
  ...
}: {
  environment.etc = {
    "nixfleet-harness/ca.pem".source = "${testCerts}/ca.pem";
    "nixfleet-harness/cp-cert.pem".source = "${testCerts}/cp-cert.pem";
    "nixfleet-harness/cp-key.pem".source = "${testCerts}/cp-key.pem";
    "nixfleet-harness/signed/canonical.json".source = "${signedFixture}/canonical.json";
    "nixfleet-harness/signed/canonical.json.sig".source = "${signedFixture}/canonical.json.sig";
  };

  networking.firewall.allowedTCPPorts = [8443];

  systemd.services.harness-cp = let
    responder = pkgs.writeShellScript "harness-cp-signed-responder" ''
      set -eu
      export PATH=${lib.makeBinPath [pkgs.coreutils]}:$PATH

      # First line is "METHOD PATH VERSION\r". Drop the trailing CR.
      IFS=' ' read -r _method path _version
      path="''${path%$(printf '\r')}"

      dir=/etc/nixfleet-harness/signed
      case "$path" in
        /canonical.json)     file="$dir/canonical.json";     ct="application/json" ;;
        /canonical.json.sig) file="$dir/canonical.json.sig"; ct="application/octet-stream" ;;
        *)
          printf 'HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'
          exit 0
          ;;
      esac

      len=$(stat -c %s "$file")
      printf 'HTTP/1.1 200 OK\r\nContent-Type: %s\r\nContent-Length: %d\r\nConnection: close\r\n\r\n' "$ct" "$len"
      cat "$file"
    '';
    socatOpts = lib.concatStringsSep "," [
      "OPENSSL-LISTEN:8443"
      "reuseaddr"
      "fork"
      "cert=/etc/nixfleet-harness/cp-cert.pem"
      "key=/etc/nixfleet-harness/cp-key.pem"
      "cafile=/etc/nixfleet-harness/ca.pem"
      "verify=1"
    ];
    launcher = pkgs.writeShellScript "harness-cp-signed-launcher" ''
      exec ${pkgs.socat}/bin/socat -v "${socatOpts}" "EXEC:${responder}"
    '';
  in {
    description = "Nixfleet harness CP stub (serves signed fixture — canonical.json + .sig)";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    serviceConfig = {
      Type = "simple";
      ExecStart = "${launcher}";
      Restart = "on-failure";
      RestartSec = 2;
    };
  };

  system.stateVersion = lib.mkDefault "24.11";
}
