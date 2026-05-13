{
  pkgs,
  seedSalt ? "nixfleet-harness-test-seed-2026",
}: let
  seedHex = builtins.substring 0 64 (builtins.hashString "sha256" seedSalt);

  keygen = pkgs.writers.writePython3 "ed25519-pkcs8-from-seed" {} ''
    import base64
    import sys

    seed = bytes.fromhex(sys.argv[1])
    assert len(seed) == 32
    der = bytes.fromhex("302e020100300506032b657004220420") + seed
    with open(sys.argv[2], "w") as f:
        f.write("-----BEGIN PRIVATE KEY-----\n")
        f.write(base64.b64encode(der).decode("ascii") + "\n")
        f.write("-----END PRIVATE KEY-----\n")
  '';
in
  pkgs.runCommand "nixfleet-harness-signed-seed-key" {
    nativeBuildInputs = [pkgs.openssl];
    inherit seedHex;
  } ''
    set -euo pipefail
    mkdir -p "$out"
    ${keygen} "$seedHex" "$out/privkey.pem"
    openssl pkey -in "$out/privkey.pem" -pubout -outform DER -out pubkey.spki.der
    tail -c 32 pubkey.spki.der | base64 -w0 > "$out/pubkey.b64"
  ''
