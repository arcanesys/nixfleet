# vm-fleet-mtls-missing - A3
#
# When the control plane has `tls.clientCa` set, every incoming TLS
# connection must present a client certificate signed by the fleet CA.
# A client that only has the CA certificate (so it can verify the server)
# but no client key pair must be rejected at the TLS handshake layer -
# the application handler never runs.
#
# This is a pure transport-layer test: we use raw curl rather than the
# agent or CLI so we observe the handshake error directly.
#
# Topology: `cp` (mTLS required) + `unauth` (has CA only, no client cert
# for the failing calls; a valid cert is copied over after for the
# positive control).
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
  name = "vm-fleet-mtls-missing";

  nodes.cp = mkCpNode {inherit testCerts;};

  # The unauth node is a plain client (no agent). mkAgentNode would
  # install services.nixfleet-agent which we explicitly don't want;
  # keep the inline module.
  nodes.unauth = mkTestNode {
    hostSpecValues = defaultTestSpec // {hostName = "unauth";};
    extraModules = [
      {
        security.pki.certificateFiles = ["${testCerts}/ca.pem"];

        # The CA is available so curl can verify the CP's server cert.
        # The client cert/key are ALSO installed on disk, but the
        # failing-path curls deliberately do not pass --cert / --key.
        # They are only used in the final positive control.
        environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
        environment.etc."nixfleet-tls/unauth-cert.pem".source = "${testCerts}/unauth-cert.pem";
        environment.etc."nixfleet-tls/unauth-key.pem".source = "${testCerts}/unauth-key.pem";

        environment.systemPackages = [pkgs.curl];
      }
    ];
  };

  testScript = ''
    # ------------------------------------------------------------------
    # Step 1 - Start CP with mTLS required (clientCa set)
    # ------------------------------------------------------------------
    cp.start()
    cp.wait_for_unit("nixfleet-control-plane.service")
    cp.wait_for_open_port(8080)

    # ------------------------------------------------------------------
    # Step 2 - Start the unauth node
    # ------------------------------------------------------------------
    unauth.start()
    unauth.wait_for_unit("multi-user.target")

    # ------------------------------------------------------------------
    # Step 3 - curl /health WITHOUT a client cert must fail at TLS
    #
    # We use `--fail-with-body` so curl still returns non-zero on any
    # non-2xx, and `-v` so the TLS diagnostics land in stderr. The
    # capture goes through `2>&1` piped into a file because
    # testers.runNixOSTest runs each command in its own shell.
    # ------------------------------------------------------------------
    health_stderr = unauth.fail(
        "curl -v --cacert /etc/nixfleet-tls/ca.pem "
        "https://cp:8080/health 2>&1"
    )

    # The exact phrasing depends on the TLS library curl was linked
    # against (OpenSSL vs GnuTLS vs Rustls). Accept any of the common
    # markers that identify a handshake/alert-level failure.
    tls_markers = [
        "alert",
        "handshake",
        "certificate required",
        "peer did not return a certificate",
        "sslv3 alert",
        "tlsv13 alert",
        "SSL_ERROR",
        "SSL routines",
    ]
    assert any(m in health_stderr for m in tls_markers), (
        f"expected a TLS handshake error in stderr, got: {health_stderr!r}"
    )

    # ------------------------------------------------------------------
    # Step 4 - The agent report endpoint is ALSO rejected at the
    # TLS layer, before the handler ever runs.
    # ------------------------------------------------------------------
    report_stderr = unauth.fail(
        "curl -v --cacert /etc/nixfleet-tls/ca.pem "
        "-X POST https://cp:8080/api/v1/machines/web-01/report "
        "-H 'Content-Type: application/json' "
        "-d '{}' 2>&1"
    )
    assert any(m in report_stderr for m in tls_markers), (
        f"expected a TLS handshake error on /report, got: {report_stderr!r}"
    )

    # ------------------------------------------------------------------
    # Step 5 (positive control) - The SAME curl with a valid client
    # cert signed by the fleet CA must succeed at the TLS layer.
    #
    # For /health we expect HTTP 200 (public endpoint).
    # For /report we expect a non-TLS response (any HTTP status is
    # fine - what matters is that the handshake completed).
    # ------------------------------------------------------------------
    health_ok = unauth.succeed(
        "curl -sf --cacert /etc/nixfleet-tls/ca.pem "
        "--cert /etc/nixfleet-tls/unauth-cert.pem "
        "--key /etc/nixfleet-tls/unauth-key.pem "
        "https://cp:8080/health"
    )
    assert health_ok.strip(), f"expected non-empty /health body, got {health_ok!r}"

    # `curl -o /dev/null -w '%{http_code}'` prints just the status
    # code. Any non-000 status proves the handshake succeeded and
    # an HTTP response came back; TLS failures would yield 000.
    report_status = unauth.succeed(
        "curl -s -o /dev/null -w '%{http_code}' "
        "--cacert /etc/nixfleet-tls/ca.pem "
        "--cert /etc/nixfleet-tls/unauth-cert.pem "
        "--key /etc/nixfleet-tls/unauth-key.pem "
        "-X POST https://cp:8080/api/v1/machines/web-01/report "
        "-H 'Content-Type: application/json' "
        "-d '{}'"
    ).strip()
    assert report_status != "000", (
        f"expected a real HTTP status (TLS handshake OK) on /report, "
        f"got {report_status!r}"
    )
  '';
}
