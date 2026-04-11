# Shared TLS certificate derivation for VM fleet tests.
#
# Produces a fleet CA, a CP server cert, and one client cert per hostname.
# Deterministic and cached — the same `hostnames` list yields the same
# derivation across tests.
#
# Usage:
#   let
#     mkTlsCerts = import ./_lib/tls-certs.nix {inherit pkgs lib;};
#     testCerts = mkTlsCerts {hostnames = ["web-01" "web-02" "db-01"];};
#   in
#     ...
{
  pkgs,
  lib,
}: {hostnames ? ["web-01" "web-02" "db-01"]}:
pkgs.runCommand "nixfleet-fleet-test-certs" {
  nativeBuildInputs = [pkgs.openssl];
} ''
  mkdir -p $out

  # Fleet CA (self-signed, EC P-256)
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
    -keyout $out/ca-key.pem -out $out/ca.pem -days 365 -nodes \
    -subj '/CN=nixfleet-test-ca'

  # CP server cert (CN=cp, SAN includes cp + localhost for test curl)
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
    -keyout $out/cp-key.pem -out $out/cp-csr.pem -nodes \
    -subj '/CN=cp' \
    -addext 'subjectAltName=DNS:cp,DNS:localhost'
  openssl x509 -req -in $out/cp-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
    -CAcreateserial -out $out/cp-cert.pem -days 365 \
    -copy_extensions copyall

  # Agent client certs (CN = hostname)
  ${lib.concatMapStringsSep "\n" (h: ''
      openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
        -keyout $out/${h}-key.pem -out $out/${h}-csr.pem -nodes \
        -subj "/CN=${h}"
      openssl x509 -req -in $out/${h}-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
        -CAcreateserial -out $out/${h}-cert.pem -days 365
    '')
    hostnames}

  rm -f $out/*-csr.pem $out/*.srl
''
