{
  lib,
  pkgs,
  testCerts,
  signedFixture,
  cpPkg,
  revocationsFixture ? null,
  ...
}: let
  hasRevocations = revocationsFixture != null;
in {
  imports = [
    ../../../contracts/trust.nix
    ../../../contracts/persistence.nix
    ../../../modules/scopes/nixfleet/_control-plane.nix
  ];

  environment.etc =
    {
      "nixfleet-cp/ca.pem".source = "${testCerts}/ca.pem";
      "nixfleet-cp/cp-cert.pem".source = "${testCerts}/cp-cert.pem";
      "nixfleet-cp/cp-key.pem".source = "${testCerts}/cp-key.pem";
      "nixfleet-cp/fleet-ca-cert.pem".source = "${testCerts}/ca.pem";
      "nixfleet-cp/fleet-ca-key.pem".source = "${testCerts}/ca-key.pem";
    }
    // lib.optionalAttrs hasRevocations {
      "nixfleet-cp-static/revocations.json".source = "${revocationsFixture}/revocations.json";
      "nixfleet-cp-static/revocations.json.sig".source = "${revocationsFixture}/revocations.json.sig";
    };

  systemd.services.harness-revocations-server = lib.mkIf hasRevocations {
    description = "Static HTTP server for the harness revocations sidecar";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    serviceConfig = {
      Type = "simple";
      ExecStart = "${pkgs.python3}/bin/python3 -m http.server 9090 --directory /etc/nixfleet-cp-static --bind 127.0.0.1";
      Restart = "on-failure";
      RestartSec = 2;
    };
  };

  # LOADBEARING: CP poll loop needs a reachable upstream on first tick (revocations sidecar must start before CP).
  systemd.services.nixfleet-control-plane.after =
    lib.mkIf hasRevocations ["harness-revocations-server.service"];

  networking.firewall.allowedTCPPorts = lib.optional hasRevocations 9090;

  services.nixfleet-control-plane =
    {
      enable = true;
      package = cpPkg;
      listen = "0.0.0.0:8443";
      openFirewall = true;

      artifactPath = "${signedFixture}/canonical.json";
      signaturePath = "${signedFixture}/canonical.json.sig";
      trustFile = "${signedFixture}/test-trust.json";

      observedPath = "/var/lib/nixfleet-cp/observed.json";

      tls = {
        cert = "/etc/nixfleet-cp/cp-cert.pem";
        key = "/etc/nixfleet-cp/cp-key.pem";
        clientCa = "/etc/nixfleet-cp/ca.pem";
      };

      fleetCaCert = "/etc/nixfleet-cp/fleet-ca-cert.pem";
      fleetCaKey = "/etc/nixfleet-cp/fleet-ca-key.pem";
      auditLogPath = "/var/lib/nixfleet-cp/audit.log";
      dbPath = "/var/lib/nixfleet-cp/state.db";

      freshnessWindowMinutes = 43200;
    }
    // lib.optionalAttrs hasRevocations {
      revocationsSource = {
        artifactUrl = "http://127.0.0.1:9090/revocations.json";
        signatureUrl = "http://127.0.0.1:9090/revocations.json.sig";
      };
    };

  system.stateVersion = lib.mkDefault "24.11";
}
