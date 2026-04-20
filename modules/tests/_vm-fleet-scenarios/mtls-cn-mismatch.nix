# vm-fleet-mtls-cn-mismatch - mTLS CN validation
#
# Companion to mtls-missing. That test pins the CA-boundary
# enforcement at the TLS layer: a client without a fleet-CA-signed
# cert is rejected at the handshake. This test pins the per-agent
# enforcement on TOP of the CA boundary: a client WITH a valid
# fleet-CA-signed cert can still be rejected at the application
# layer if its CN does not match the {id} path segment.
#
# The defense closes the impersonation gap: without CN validation,
# an agent whose cert leaks (or whose private key is exfiltrated)
# could hit any other agent's endpoints, since the CA alone says
# only "you are a member of the fleet" - not "you are this
# specific agent".
#
# Topology: cp (mTLS required) + unauth (has a valid cert with
# CN="unauth" but tries to call /machines/web-01/...).
{
  pkgs,
  inputs,
  mkTestNode,
  defaultTestSpec,
  mkCpNode,
  testCerts,
  ...
}:
pkgs.testers.runNixOSTest {
  node.specialArgs = {inherit inputs;};
  name = "vm-fleet-mtls-cn-mismatch";

  nodes.cp = mkCpNode {inherit testCerts;};

  nodes.unauth = mkTestNode {
    hostSpecValues = defaultTestSpec // {hostName = "unauth";};
    extraModules = [
      {
        security.pki.certificateFiles = ["${testCerts}/ca.pem"];
        environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
        environment.etc."nixfleet-tls/unauth-cert.pem".source = "${testCerts}/unauth-cert.pem";
        environment.etc."nixfleet-tls/unauth-key.pem".source = "${testCerts}/unauth-key.pem";
        environment.systemPackages = [pkgs.curl];
      }
    ];
  };

  testScript = ''
    # ------------------------------------------------------------------
    # Step 1 - Start CP with mTLS required (clientCa set in mkCpNode).
    # ------------------------------------------------------------------
    cp.start()
    cp.wait_for_unit("nixfleet-control-plane.service")
    cp.wait_for_open_port(8080)

    unauth.start()
    unauth.wait_for_unit("multi-user.target")

    # ------------------------------------------------------------------
    # Step 2 - POST to /machines/web-01/report with a CERT WHOSE CN IS
    # "unauth". The TLS handshake completes (cert is signed by the
    # fleet CA). The auth_cn middleware then reads the peer cert from
    # the request extension, extracts CN="unauth", compares against
    # path id="web-01", and rejects with 403.
    # ------------------------------------------------------------------
    mismatch_status = unauth.succeed(
        "curl -s -o /dev/null -w '%{http_code}' "
        "--cacert /etc/nixfleet-tls/ca.pem "
        "--cert /etc/nixfleet-tls/unauth-cert.pem "
        "--key /etc/nixfleet-tls/unauth-key.pem "
        "-X POST https://cp:8080/api/v1/machines/web-01/report "
        "-H 'Content-Type: application/json' "
        "-d '{\"machine_id\":\"web-01\",\"current_generation\":\"x\","
        "\"success\":true,\"message\":\"impersonation\","
        "\"timestamp\":\"2026-04-11T00:00:00Z\",\"tags\":[]}'"
    ).strip()
    assert mismatch_status == "403", (
        f"expected 403 (CN mismatch), got HTTP {mismatch_status!r}. "
        f"If this is 200 or 2xx the auth_cn middleware is broken or "
        f"not wired on agent routes."
    )

    # ------------------------------------------------------------------
    # Step 3 (positive control) - Same cert, but the path now matches
    # the cert's CN ("unauth"). The middleware accepts and the request
    # reaches the report handler. We don't care about the handler's
    # response code - only that it is NOT 403 (which would mean the
    # middleware also blocked the matching case, indicating it's
    # rejecting blindly).
    # ------------------------------------------------------------------
    match_status = unauth.succeed(
        "curl -s -o /dev/null -w '%{http_code}' "
        "--cacert /etc/nixfleet-tls/ca.pem "
        "--cert /etc/nixfleet-tls/unauth-cert.pem "
        "--key /etc/nixfleet-tls/unauth-key.pem "
        "-X POST https://cp:8080/api/v1/machines/unauth/report "
        "-H 'Content-Type: application/json' "
        "-d '{\"machine_id\":\"unauth\",\"current_generation\":\"x\","
        "\"success\":true,\"message\":\"matching\","
        "\"timestamp\":\"2026-04-11T00:00:00Z\",\"tags\":[]}'"
    ).strip()
    assert match_status != "403", (
        f"expected non-403 (CN matches path), got HTTP {match_status!r}. "
        f"The middleware must not block requests where the cert CN "
        f"matches the path machine_id."
    )

    # ------------------------------------------------------------------
    # Step 4 - GET /machines/web-01/desired-generation also enforces
    # CN validation. Same cert (CN="unauth"), different path (web-01).
    # ------------------------------------------------------------------
    desired_status = unauth.succeed(
        "curl -s -o /dev/null -w '%{http_code}' "
        "--cacert /etc/nixfleet-tls/ca.pem "
        "--cert /etc/nixfleet-tls/unauth-cert.pem "
        "--key /etc/nixfleet-tls/unauth-key.pem "
        "https://cp:8080/api/v1/machines/web-01/desired-generation"
    ).strip()
    assert desired_status == "403", (
        f"expected 403 on GET /machines/web-01/desired-generation with "
        f"CN=unauth cert, got HTTP {desired_status!r}"
    )
  '';
}
