{
  pkgs,
  signedFixture,
  verifyArtifactPkg,
  ...
}: let
  now = signedFixture.now;
  freshnessWindowSecs = 2592000;
in
  pkgs.runCommand "fleet-harness-corruption-rejection" {} ''
    set -euo pipefail

    ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact ${signedFixture}/canonical.json \
      --signature ${signedFixture}/canonical.json.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${now} \
      --freshness-window-secs ${toString freshnessWindowSecs}

    # Offset 50 keeps the JSON loosely parseable so verify reaches the sig step.
    cp ${signedFixture}/canonical.json tampered-canonical.json
    chmod +w tampered-canonical.json
    printf '\x01' | dd of=tampered-canonical.json bs=1 count=1 seek=50 \
      conv=notrunc 2>/dev/null
    if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact tampered-canonical.json \
      --signature ${signedFixture}/canonical.json.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${now} \
      --freshness-window-secs ${toString freshnessWindowSecs}; then
      echo "FAIL: tampered canonical.json was accepted by verify-artifact" >&2
      exit 1
    fi

    cp ${signedFixture}/canonical.json.sig tampered.sig
    chmod +w tampered.sig
    printf '\x01' | dd of=tampered.sig bs=1 count=1 seek=10 \
      conv=notrunc 2>/dev/null
    if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact ${signedFixture}/canonical.json \
      --signature tampered.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${now} \
      --freshness-window-secs ${toString freshnessWindowSecs}; then
      echo "FAIL: tampered signature was accepted by verify-artifact" >&2
      exit 1
    fi

    touch "$out"
  ''
