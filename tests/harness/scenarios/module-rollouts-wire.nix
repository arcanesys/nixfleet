# LOADBEARING: boots the real `services.nixfleet-control-plane` module (not cp-real.nix unit) so ExecStart-construction regressions surface.
{
  pkgs,
  inputs,
  rolloutManifestFixture,
  signedFixture,
  testCerts,
  ...
}: let
  # GOTCHA: CP serves <rolloutId>.json{,.sig}; rolloutId is sha256 of canonical bytes, only known at build time.
  rolloutsDir =
    pkgs.runCommand "harness-rollouts-dir" {
      nativeBuildInputs = [pkgs.coreutils];
    } ''
      mkdir -p "$out"
      rid=$(cat ${rolloutManifestFixture}/rollout-id)
      cp ${rolloutManifestFixture}/manifest.canonical.json     "$out/$rid.json"
      cp ${rolloutManifestFixture}/manifest.canonical.json.sig "$out/$rid.json.sig"
    '';
in
  pkgs.testers.runNixOSTest {
    name = "fleet-harness-module-rollouts-wire";
    meta.timeout = 300;

    node.specialArgs = {inherit inputs;};

    nodes.host = {pkgs, ...}: {
      imports = [
        ../../../contracts/trust.nix
        ../../../contracts/persistence.nix
        ../../../modules/scopes/nixfleet/_control-plane.nix
      ];

      services.nixfleet-control-plane = {
        enable = true;
        listen = "0.0.0.0:8443";
        openFirewall = true;

        artifactPath = "${signedFixture}/canonical.json";
        signaturePath = "${signedFixture}/canonical.json.sig";
        trustFile = "${signedFixture}/test-trust.json";

        tls = {
          cert = "${testCerts}/cp-cert.pem";
          key = "${testCerts}/cp-key.pem";
          clientCa = "${testCerts}/ca.pem";
        };

        rolloutsDir = "${rolloutsDir}";
      };

      environment.etc = {
        "nixfleet-test/rollout-id".source = "${rolloutManifestFixture}/rollout-id";
        "nixfleet-test/expected.json".source = "${rolloutManifestFixture}/manifest.canonical.json";
        "nixfleet-test/expected.sig".source = "${rolloutManifestFixture}/manifest.canonical.json.sig";
        "nixfleet-test/agent-cert.pem".source = "${testCerts}/agent-01-cert.pem";
        "nixfleet-test/agent-key.pem".source = "${testCerts}/agent-01-key.pem";
        "nixfleet-test/ca.pem".source = "${testCerts}/ca.pem";
      };

      environment.systemPackages = [pkgs.curl];
    };

    testScript = ''
      host.start()
      host.wait_for_unit("multi-user.target")
      host.wait_for_unit("nixfleet-control-plane.service")
      host.wait_for_open_port(8443)

      rid = host.succeed("cat /etc/nixfleet-test/rollout-id").strip()

      curl_base = (
          "curl -sS "
          "--cacert /etc/nixfleet-test/ca.pem "
          "--cert /etc/nixfleet-test/agent-cert.pem "
          "--key /etc/nixfleet-test/agent-key.pem "
          "--resolve cp:8443:127.0.0.1 "
      )

      host.succeed(
          f"{curl_base} --fail "
          f"https://cp:8443/v1/rollouts/{rid} -o /tmp/got.json"
      )
      host.succeed("diff -u /etc/nixfleet-test/expected.json /tmp/got.json")

      host.succeed(
          f"{curl_base} --fail "
          f"https://cp:8443/v1/rollouts/{rid}/sig -o /tmp/got.sig"
      )
      host.succeed("cmp /etc/nixfleet-test/expected.sig /tmp/got.sig")

      bogus = "0" * 64
      code = host.succeed(
          f"{curl_base} -o /dev/null -w '%{{http_code}}' "
          f"https://cp:8443/v1/rollouts/{bogus}"
      ).strip()
      assert code == "404", f"expected 404 for unknown rolloutId, got {code}"

      print(
          "fleet-harness-module-rollouts-wire: services.nixfleet-control-plane "
          "module threaded `rolloutsDir` through ExecStart to the running CP; "
          "GET /v1/rollouts/<id>{,/sig} served fixture bytes byte-for-byte; "
          "unknown rid returned 404."
      )
    '';
  }
