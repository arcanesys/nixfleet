# LOADBEARING: validates `meta.signedAt > now + CLOCK_SKEW_SLACK_SECS` is
# rejected as `FutureDated` - a CI key compromise (pre-signed manifest with
# future signedAt) shouldn't pass freshness; rotate via `reject_before`.
{
  pkgs,
  signedFixture,
  verifyArtifactPkg,
  ...
}: let
  signedAt = "2026-05-01T00:00:00Z";
  twoDaysBefore = "2026-04-29T00:00:00Z";
  thirtySecondsBefore = "2026-04-30T23:59:30Z";
  exactly = signedAt;
  thirtySecondsAfter = "2026-05-01T00:00:30Z";
  freshnessWindowSecs = 86400;
in
  pkgs.runCommand "fleet-harness-future-dated-rejection" {} ''
    set -euo pipefail

    echo "step 1: dt=+2d, expect reject..."
    if ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
         --artifact ${signedFixture}/canonical.json \
         --signature ${signedFixture}/canonical.json.sig \
         --trust-file ${signedFixture}/test-trust.json \
         --now ${twoDaysBefore} \
         --freshness-window-secs ${toString freshnessWindowSecs} \
         2> step1.stderr; then
      echo "FAIL: dt=+2d future-dated artifact accepted" >&2
      cat step1.stderr >&2 || true
      exit 1
    fi
    if ! grep -q 'future-dated artifact' step1.stderr; then
      echo "FAIL: missing 'future-dated artifact' in stderr" >&2
      cat step1.stderr >&2
      exit 1
    fi
    if ! grep -q 'clock skew tolerance is 60s' step1.stderr; then
      echo "FAIL: missing '60s' tolerance copy" >&2
      cat step1.stderr >&2
      exit 1
    fi
    echo "step 1: dt=+2d rejected"

    echo "step 2: dt=+30s, expect accept (within 60s slack)..."
    ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact ${signedFixture}/canonical.json \
      --signature ${signedFixture}/canonical.json.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${thirtySecondsBefore} \
      --freshness-window-secs ${toString freshnessWindowSecs}
    echo "step 2: dt=+30s accepted"

    echo "step 3: dt=0, expect accept..."
    ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact ${signedFixture}/canonical.json \
      --signature ${signedFixture}/canonical.json.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${exactly} \
      --freshness-window-secs ${toString freshnessWindowSecs}
    echo "step 3: dt=0 accepted"

    # Pair to step 2: same 60s slack on the past side proves symmetry.
    echo "step 4: dt=-30s, expect accept (past-slack mirror)..."
    ${verifyArtifactPkg}/bin/nixfleet-verify-artifact artifact \
      --artifact ${signedFixture}/canonical.json \
      --signature ${signedFixture}/canonical.json.sig \
      --trust-file ${signedFixture}/test-trust.json \
      --now ${thirtySecondsAfter} \
      --freshness-window-secs ${toString freshnessWindowSecs}
    echo "step 4: dt=-30s accepted"

    touch "$out"
  ''
