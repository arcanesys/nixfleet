{
  pkgs,
  rolloutManifestFixture,
  verifyArtifactPkg,
  ...
}:
pkgs.runCommand "fleet-harness-manifest-tamper-rejection" {} ''
  set -euo pipefail

  rid=$(cat ${rolloutManifestFixture}/rollout-id)
  signedAt=$(cat ${rolloutManifestFixture}/signed-at)

  ${verifyArtifactPkg}/bin/nixfleet-verify-artifact rollout-manifest \
    --manifest ${rolloutManifestFixture}/manifest.canonical.json \
    --signature ${rolloutManifestFixture}/manifest.canonical.json.sig \
    --trust-file ${rolloutManifestFixture}/trust.json \
    --now "$signedAt" \
    --freshness-window-secs 86400 \
    --rollout-id "$rid"

  cp ${rolloutManifestFixture}/manifest.canonical.json tampered-manifest.json
  chmod +w tampered-manifest.json
  printf '\x01' | dd of=tampered-manifest.json bs=1 count=1 seek=50 \
    conv=notrunc 2>/dev/null
  if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact rollout-manifest \
       --manifest tampered-manifest.json \
       --signature ${rolloutManifestFixture}/manifest.canonical.json.sig \
       --trust-file ${rolloutManifestFixture}/trust.json \
       --now "$signedAt" \
       --freshness-window-secs 86400 \
       --rollout-id "$rid" \
       2>/dev/null; then
    echo "FAIL: tampered manifest accepted by verify-artifact rollout-manifest" >&2
    exit 1
  fi

  cp ${rolloutManifestFixture}/manifest.canonical.json.sig tampered.sig
  chmod +w tampered.sig
  printf '\xff' | dd of=tampered.sig bs=1 count=1 seek=10 \
    conv=notrunc 2>/dev/null
  if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact rollout-manifest \
       --manifest ${rolloutManifestFixture}/manifest.canonical.json \
       --signature tampered.sig \
       --trust-file ${rolloutManifestFixture}/trust.json \
       --now "$signedAt" \
       --freshness-window-secs 86400 \
       --rollout-id "$rid" \
       2>/dev/null; then
    echo "FAIL: tampered signature accepted" >&2
    exit 1
  fi

  # Rename/swap attack: valid signature, wrong rolloutId.
  wrong_rid="9999999999999999999999999999999999999999999999999999999999999999"
  if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact rollout-manifest \
       --manifest ${rolloutManifestFixture}/manifest.canonical.json \
       --signature ${rolloutManifestFixture}/manifest.canonical.json.sig \
       --trust-file ${rolloutManifestFixture}/trust.json \
       --now "$signedAt" \
       --freshness-window-secs 86400 \
       --rollout-id "$wrong_rid" \
       2>/dev/null; then
    echo "FAIL: rolloutId mismatch accepted (rename/swap attack not detected)" >&2
    exit 1
  fi

  touch "$out"
''
