{
  lib,
  pkgs,
  testCerts,
  resolvedJsonPath,
  ...
}: {
  environment.etc = {
    "nixfleet-harness/ca.pem".source = "${testCerts}/ca.pem";
    "nixfleet-harness/cp-cert.pem".source = "${testCerts}/cp-cert.pem";
    "nixfleet-harness/cp-key.pem".source = "${testCerts}/cp-key.pem";
    "nixfleet-harness/fleet.resolved.json".source = resolvedJsonPath;
  };

  networking.firewall.allowedTCPPorts = [8443];

  systemd.services.harness-cp = let
    responder = pkgs.writeShellScript "harness-cp-responder" ''
      set -eu
      body=$(cat /etc/nixfleet-harness/fleet.resolved.json)
      len=$(printf '%s' "$body" | wc -c)
      printf 'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' "$len" "$body"
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
    launcher = pkgs.writeShellScript "harness-cp-launcher" ''
      exec ${pkgs.socat}/bin/socat -v "${socatOpts}" "EXEC:${responder}"
    '';
  in {
    description = "Nixfleet harness CP stub (serves fleet.resolved.json)";
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
