{
  pkgs,
  nixfleet-canonicalize,
  jsonContent,
  name,
  seedSalt ? "nixfleet-harness-test-seed-2026",
}: let
  seedHex = builtins.substring 0 64 (builtins.hashString "sha256" seedSalt);

  # FOOTGUN: hand-built PKCS#8 DER; openssl 3 genpkey won't accept a caller-supplied seed.
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
  pkgs.runCommand name {
    nativeBuildInputs = [pkgs.openssl];
    passAsFile = ["jsonContent"];
    inherit jsonContent seedHex;
  } ''
    set -euo pipefail
    mkdir -p "$out"
    ${keygen} "$seedHex" privkey.pem

    cp "$jsonContentPath" stamped.json
    ${nixfleet-canonicalize}/bin/nixfleet-canonicalize \
      < stamped.json > "$out/canonical.json"
    openssl pkeyutl -sign -rawin -inkey privkey.pem \
      -in "$out/canonical.json" -out "$out/canonical.json.sig"
    siglen=$(stat -c %s "$out/canonical.json.sig")
    [ "$siglen" -eq 64 ] || { echo "bad sig length: $siglen" >&2; exit 1; }

    openssl pkey -in privkey.pem -pubout -outform DER -out pubkey.spki.der
    tail -c 32 pubkey.spki.der | base64 -w0 > "$out/pubkey.b64"
  ''
