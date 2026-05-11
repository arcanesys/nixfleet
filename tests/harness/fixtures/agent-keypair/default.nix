# Deterministic ed25519 keypair for a harness agent.
#
# Seed is `hashString "sha256" (seedSalt + "/" + hostName)` so each agent
# gets a distinct, reproducible keypair without requiring a separate
# seedSalt per call site.
#
# Outputs (all under $out):
#   private.pem      — PKCS#8 PEM (consumed by `openssl req` when a test
#                      mints a CSR with this keypair)
#   private.openssh  — OpenSSH PEM private key (consumed by the agent's
#                      evidence_signer via /etc/ssh/ssh_host_ed25519_key;
#                      also accepted by anything else that parses
#                      OpenSSH-format ed25519 keys)
#   public.openssh   — single-line OpenSSH-format public key (consumed
#                      by signedFixture as hosts.<hostName>.pubkey; the
#                      CP binds CSR + attestation pubkeys against this
#                      per #43)
{
  pkgs,
  hostName,
  seedSalt ? "nixfleet-harness-agent-keypair-2026",
  derivationName ? "nixfleet-harness-agent-keypair-${hostName}",
}: let
  seedHex =
    builtins.substring 0 64
    (builtins.hashString "sha256" "${seedSalt}/${hostName}");

  keygen =
    pkgs.writers.writePython3 "ed25519-keypair-from-seed-${hostName}" {
      libraries = [pkgs.python3Packages.cryptography];
    } ''
      import base64
      import struct
      import sys

      from cryptography.hazmat.primitives.asymmetric.ed25519 import (
          Ed25519PrivateKey,
      )
      from cryptography.hazmat.primitives.serialization import (
          Encoding, PrivateFormat, NoEncryption, PublicFormat,
      )

      seed = bytes.fromhex(sys.argv[1])
      assert len(seed) == 32, f"expected 32-byte seed, got {len(seed)}"
      private = Ed25519PrivateKey.from_private_bytes(seed)
      public = private.public_key()
      raw_pub = public.public_bytes(Encoding.Raw, PublicFormat.Raw)
      assert len(raw_pub) == 32

      # PKCS#8 PEM (consumed by openssl req).
      with open(sys.argv[2], "wb") as f:
          f.write(private.private_bytes(
              Encoding.PEM, PrivateFormat.PKCS8, NoEncryption()
          ))

      # OpenSSH PEM private (consumed by evidence_signer).
      with open(sys.argv[3], "wb") as f:
          f.write(private.private_bytes(
              Encoding.PEM, PrivateFormat.OpenSSH, NoEncryption()
          ))

      # OpenSSH-format pubkey: "ssh-ed25519 <b64-blob>", where blob is
      # [4B len=11][b"ssh-ed25519"][4B len=32][raw_pub].
      blob = (
          struct.pack(">I", 11)
          + b"ssh-ed25519"
          + struct.pack(">I", 32)
          + raw_pub
      )
      openssh = b"ssh-ed25519 " + base64.b64encode(blob)
      with open(sys.argv[4], "wb") as f:
          f.write(openssh)
    '';
in
  pkgs.runCommand derivationName {
    nativeBuildInputs = [pkgs.coreutils];
    inherit seedHex;
  } ''
    set -euo pipefail
    mkdir -p "$out"
    ${keygen} "$seedHex" \
      "$out/private.pem" \
      "$out/private.openssh" \
      "$out/public.openssh"
  ''
