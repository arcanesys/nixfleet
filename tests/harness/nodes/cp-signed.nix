# tests/harness/nodes/cp-signed.nix
#
# Signed-roundtrip CP node. Serves two files from the Phase 2 signed
# fixture over mTLS on :8443, routed by request path:
#
#   GET /canonical.json      -> application/json
#   GET /canonical.json.sig  -> application/octet-stream (raw 64 bytes)
#
# Implementation: Python stdlib `http.server` wrapped in `ssl`. An
# earlier shell-over-socat implementation truncated binary responses:
# even with request draining and `exec cat`, the socat EXEC pipeline
# consistently delivered 54 of the signature's 64 bytes to the client,
# though socat's own `-v` log confirmed the full 64 bytes had been
# forwarded. The exact mangling point was never identified; the fix
# was to stop debugging shell and ship a binary-safe server.
#
# TODO: retire this module when `services.nixfleet-control-plane`
# gains the artifact-serve endpoint. The wire shape (two paths, mTLS,
# path-routed) is the real CP's contract.
{
  pkgs,
  lib,
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
    server = pkgs.writers.writePython3 "harness-cp-signed-server" {} ''
      import ssl
      import sys
      from http.server import BaseHTTPRequestHandler, HTTPServer

      BASE = "/etc/nixfleet-harness/signed"
      ROUTES = {
          "/canonical.json": (f"{BASE}/canonical.json", "application/json"),
          "/canonical.json.sig": (f"{BASE}/canonical.json.sig", "application/octet-stream"),
      }


      class Handler(BaseHTTPRequestHandler):
          protocol_version = "HTTP/1.1"

          def do_GET(self):
              entry = ROUTES.get(self.path)
              if entry is None:
                  self.send_response(404)
                  self.send_header("Content-Length", "0")
                  self.send_header("Connection", "close")
                  self.end_headers()
                  return
              path, ctype = entry
              with open(path, "rb") as f:
                  data = f.read()
              self.send_response(200)
              self.send_header("Content-Type", ctype)
              self.send_header("Content-Length", str(len(data)))
              self.send_header("Connection", "close")
              self.end_headers()
              self.wfile.write(data)

          def log_message(self, fmt, *args):
              sys.stderr.write("harness-cp: " + (fmt % args) + "\n")


      def main():
          ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
          ctx.load_cert_chain(
              certfile="/etc/nixfleet-harness/cp-cert.pem",
              keyfile="/etc/nixfleet-harness/cp-key.pem",
          )
          ctx.load_verify_locations(cafile="/etc/nixfleet-harness/ca.pem")
          ctx.verify_mode = ssl.CERT_REQUIRED

          httpd = HTTPServer(("0.0.0.0", 8443), Handler)
          httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
          httpd.serve_forever()


      main()
    '';
  in {
    description = "Nixfleet harness CP stub (serves signed fixture — canonical.json + .sig, Python mTLS)";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    serviceConfig = {
      Type = "simple";
      ExecStart = "${server}";
      Restart = "on-failure";
      RestartSec = 2;
    };
  };

  system.stateVersion = lib.mkDefault "24.11";
}
