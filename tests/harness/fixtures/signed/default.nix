# tests/harness/fixtures/signed/default.nix
#
# Deterministic signed-fixture derivation for the Phase 2 microvm harness.
# Produces an ed25519-signed `fleet.resolved` artifact at build time,
# along with the raw verify key and a `test-trust.json` file shaped per
# `docs/trust-root-flow.md` §3.4.
#
# Determinism. Every byte in the output is a pure function of this file's
# inputs:
# - The fleet declaration is hand-authored below (static).
# - `meta.{signedAt, ciCommit, signatureAlgorithm}` are hardcoded.
# - The ed25519 keypair is derived from a fixed 32-byte seed via PKCS#8
#   ASN.1 wrapping — see `keygenHelper` below for the exact method.
# - `nixfleet-canonicalize` is a pinned flake package (serde_jcs 0.2).
#
# Consumed by (future):
# - `tests/harness/scenarios/signed-roundtrip.nix` (Phase 2 PR(b)) —
#   serves `canonical.json` + `canonical.json.sig` from the harness CP
#   stub and injects `test-trust.json` into the agent microVM.
# - `crates/nixfleet-verify-artifact` (Phase 2 PR(a), Stream C) — reads
#   the three files as CLI inputs and exits 0 on valid verify.
#
# See `./README.md` for the full data-flow walk-through.
{
  lib,
  pkgs,
  nixfleet-canonicalize,
  # Path to lib/mkFleet.nix — resolved relative to the repo root. Exposed
  # as an argument so harness plumbing can swap in a stubbed mkFleet
  # without rebuilding the framework.
  mkFleetPath ? ../../../../lib/mkFleet.nix,
}: let
  # 32-byte seed derived from a fixed string per §12.2 of the Phase 2
  # entry spec. Changing the string forces a new keypair across every
  # consumer — intended for rotation scenarios that want a second seed.
  seedHex = builtins.substring 0 64 (builtins.hashString "sha256" "nixfleet-harness-test-seed-2026");

  # Frozen CI-metadata stamps. Keep in sync with the scenario's
  # `--now` / `--freshness-window-secs` inputs so verify does not trip
  # the freshness gate against wall-clock time at harness runtime.
  fixedSignedAt = "2026-05-01T00:00:00Z";
  fixedCiCommit = "0000000000000000000000000000000000000000"; # obviously a placeholder
  fixedAlgorithm = "ed25519";

  # --- Stub nixosConfiguration: satisfies mkFleet's invariant that
  # each host carries a configuration with `config.system.build.toplevel`.
  # Reused pattern from `tests/lib/mkFleet/fixtures/_stub-configuration.nix`.
  stubConfiguration = {
    config.system.build.toplevel = {
      outPath = "/nix/store/0000000000000000000000000000000000000000-stub";
      drvPath = "/nix/store/0000000000000000000000000000000000000000-stub.drv";
    };
  };

  # --- Import mkFleet directly; no flake indirection. The resolved tree
  # is a pure function of this file's contents.
  mkFleetImpl = import mkFleetPath {inherit lib;};
  inherit (mkFleetImpl) mkFleet withSignature;

  # --- Hand-authored fleet declaration for the harness. Two hosts, one
  # channel, one rollout policy. Deliberately minimal so any failure
  # during verify is wire-up, not fleet-shape.
  fleetInput = {
    hosts = {
      agent-01 = {
        system = "x86_64-linux";
        configuration = stubConfiguration;
        tags = ["harness"];
        channel = "stable";
        pubkey = null;
      };
      cp = {
        system = "x86_64-linux";
        configuration = stubConfiguration;
        tags = ["harness" "control-plane"];
        channel = "stable";
        pubkey = null;
      };
    };
    channels.stable = {
      description = "Harness signed-fixture channel.";
      rolloutPolicy = "all-at-once";
      reconcileIntervalMinutes = 30;
      signingIntervalMinutes = 60;
      freshnessWindow = 86400; # 60 days — way above 2 × signingInterval.
      compliance = {
        strict = false;
        frameworks = [];
      };
    };
    rolloutPolicies.all-at-once = {
      strategy = "all-at-once";
      waves = [
        {
          selector.all = true;
          soakMinutes = 0;
        }
      ];
      healthGate = {};
      onHealthFailure = "halt";
    };
    edges = [];
    disruptionBudgets = [];
  };

  fleet = mkFleet fleetInput;

  stamped =
    withSignature {
      signedAt = fixedSignedAt;
      ciCommit = fixedCiCommit;
      signatureAlgorithm = fixedAlgorithm;
    }
    fleet.resolved;

  # Nix's `builtins.toJSON` emits deterministic JSON (attrset keys sorted
  # lexicographically, no floats in our schema) — ideal feed for JCS.
  stampedJson = builtins.toJSON stamped;

  # Python 3 stdlib is enough to wrap a 32-byte seed into a PKCS#8
  # ed25519 private key. OpenSSL 3's `genpkey -algorithm ED25519` does
  # NOT accept a caller-provided seed (see openssl/openssl#18333); the
  # cleanest deterministic path is to hand-build the ASN.1.
  #
  # RFC 8410 §7 PKCS#8 PrivateKeyInfo for ed25519:
  #   SEQUENCE {
  #     version      INTEGER 0,
  #     algorithm    AlgorithmIdentifier { OID 1.3.101.112 (id-Ed25519) },
  #     privateKey   OCTET STRING containing OCTET STRING(seed)
  #   }
  # DER prefix (16 bytes): 302e020100300506032b657004220420
  # followed by the 32-byte seed. Total 48 bytes.
  keygenHelper = pkgs.writers.writePython3 "ed25519-pkcs8-from-seed" {} ''
    import base64
    import sys

    seed_hex = sys.argv[1]
    out_path = sys.argv[2]

    seed = bytes.fromhex(seed_hex)
    assert len(seed) == 32, f"expected 32-byte seed, got {len(seed)}"

    # RFC 8410 §7 DER prefix for a PKCS#8-wrapped ed25519 private key.
    der_prefix = bytes.fromhex("302e020100300506032b657004220420")
    der = der_prefix + seed
    b64 = base64.b64encode(der).decode("ascii")

    with open(out_path, "w") as f:
        f.write("-----BEGIN PRIVATE KEY-----\n")
        f.write(b64 + "\n")
        f.write("-----END PRIVATE KEY-----\n")
  '';
in
  pkgs.runCommand "nixfleet-harness-signed-fixture" {
    nativeBuildInputs = [pkgs.openssl];
    # Pass the resolved+stamped JSON in as a file to keep the derivation
    # hermetic (no environment-variable size limits, same bytes on every
    # builder).
    passAsFile = ["stampedJson"];
    inherit stampedJson seedHex;
  } ''
    set -euo pipefail

    mkdir -p "$out"

    # --- Step 1: derive the ed25519 keypair from the fixed seed.
    ${keygenHelper} "$seedHex" privkey.pem

    # --- Step 2: stage the stamped JSON and canonicalize it via JCS.
    cp "$stampedJsonPath" stamped.json
    ${nixfleet-canonicalize}/bin/nixfleet-canonicalize < stamped.json > "$out/canonical.json"

    # --- Step 3: ed25519-sign the canonical bytes. Raw 64-byte output.
    openssl pkeyutl \
      -sign -rawin \
      -inkey privkey.pem \
      -in "$out/canonical.json" \
      -out "$out/canonical.json.sig"

    # Sanity: ed25519 signatures are exactly 64 bytes.
    siglen=$(stat -c %s "$out/canonical.json.sig")
    if [ "$siglen" -ne 64 ]; then
      echo "unexpected signature length: $siglen bytes" >&2
      exit 1
    fi

    # --- Step 4: emit the verify pubkey. `openssl pkey -pubout -outform
    # DER` yields a 44-byte SPKI: 12-byte header + 32 raw pubkey bytes.
    # Strip the header and base64-encode the raw 32 bytes.
    openssl pkey -in privkey.pem -pubout -outform DER -out pubkey.spki.der
    pubkey_b64=$(tail -c 32 pubkey.spki.der | base64 -w0)
    printf '%s' "$pubkey_b64" > "$out/verify-pubkey.b64"

    # --- Step 5: emit test-trust.json per docs/trust-root-flow.md §3.4.
    # schemaVersion is REQUIRED per §7.4.
    cat > "$out/test-trust.json" <<EOF
    {
      "schemaVersion": 1,
      "ciReleaseKey": {
        "current": { "algorithm": "ed25519", "public": "$pubkey_b64" },
        "previous": null,
        "rejectBefore": null
      },
      "atticCacheKey": { "current": null },
      "orgRootKey": { "current": null }
    }
    EOF

    # Final shape check: four files, no more, no less.
    expected="canonical.json canonical.json.sig test-trust.json verify-pubkey.b64"
    actual=$(cd "$out" && ls | sort | tr '\n' ' ' | sed 's/ $//')
    if [ "$actual" != "$(printf '%s\n' $expected | sort | tr '\n' ' ' | sed 's/ $//')" ]; then
      echo "unexpected output files: $actual" >&2
      exit 1
    fi
  ''
