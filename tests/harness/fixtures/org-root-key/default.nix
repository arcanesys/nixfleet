# Bytes are a pure function of seedSalt. Caller stitches the trust.json.
{
  pkgs,
  seedSalt ? "nixfleet-harness-org-root-seed-2026",
  derivationName ? "nixfleet-harness-org-root-key",
}: let
  seedHex = builtins.substring 0 64 (builtins.hashString "sha256" seedSalt);

  # FOOTGUN: hand-built PKCS#8 DER; openssl 3 genpkey won't accept a caller-supplied seed.
  keygen = pkgs.writers.writePython3 "ed25519-pkcs8-from-seed-org-root" {} ''
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
  pkgs.runCommand derivationName {
    nativeBuildInputs = [pkgs.openssl pkgs.coreutils];
    inherit seedHex;
  } ''
    set -euo pipefail
    mkdir -p "$out"
    ${keygen} "$seedHex" "$out/private.pem"

    openssl pkey -in "$out/private.pem" -pubout -outform DER -out pubkey.spki.der
    tail -c 32 pubkey.spki.der | base64 -w0 > "$out/pubkey.b64"
  ''
