{
  pkgs,
  probeFixture,
  verifyArtifactPkg,
  ...
}:
pkgs.runCommand "fleet-harness-auditor-chain" {} ''
  set -euo pipefail

  ${verifyArtifactPkg}/bin/nixfleet-verify-artifact probe \
    --payload ${probeFixture}/payload.canonical.json \
    --signature ${probeFixture}/payload.sig.b64 \
    --pubkey ${probeFixture}/pubkey.openssh

  cp ${probeFixture}/payload.canonical.json tampered.json
  chmod +w tampered.json
  printf '\x00' | dd of=tampered.json bs=1 count=1 conv=notrunc 2>/dev/null
  if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact probe \
    --payload tampered.json \
    --signature ${probeFixture}/payload.sig.b64 \
    --pubkey ${probeFixture}/pubkey.openssh; then
    echo "FAIL: tampered payload was accepted by verify-artifact probe" >&2
    exit 1
  fi

  touch "$out"
''
