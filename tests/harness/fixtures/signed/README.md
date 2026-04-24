# Signed harness fixture

A deterministic, byte-stable `fleet.resolved.json` with an ed25519
signature, baked at Nix build time. Consumed by the Phase 2 signed
round-trip harness scenario and by the Rust verify CLI.

Lives at `tests/harness/fixtures/signed/` rather than under
`crates/*/tests/fixtures/` because the fixture is produced by a Nix
derivation (openssl + the pinned `nixfleet-canonicalize` binary). See
`docs/phase-2-entry-spec.md` §12.1 for the locked-in placement
rationale.

## What the derivation emits

Four files in a single `/nix/store/*-nixfleet-harness-signed-fixture`
output directory:

| File | Purpose |
|------|---------|
| `canonical.json` | JCS-canonical bytes of `fleet.resolved` with `meta.{signedAt, ciCommit, signatureAlgorithm}` stamped. The exact byte stream the signature covers. |
| `canonical.json.sig` | Raw 64-byte ed25519 signature over `canonical.json`. No DER, no armour. |
| `verify-pubkey.b64` | Base64 of the 32 raw pubkey bytes, no newline. Extracted from the DER SPKI output of `openssl pkey -pubout` by stripping the 12-byte header. |
| `test-trust.json` | Trust root JSON per `docs/trust-root-flow.md` §3.4 (`schemaVersion: 1` required per §7.4). Embeds the verify pubkey as `ciReleaseKey.current`. |

## Determinism

Every output byte is a pure function of this directory's contents.
Reproducibility is the whole point — any drift between two eval runs
signals non-determinism that will break the round-trip test.

Deterministic inputs:

- **Fleet declaration** — hand-authored inline in `default.nix`
  (`fleetInput` binding).
- **`meta` stamps** — `signedAt = "2026-05-01T00:00:00Z"`,
  `ciCommit = "0" × 40`, `signatureAlgorithm = "ed25519"`. Hardcoded.
- **Keypair** — derived from a 32-byte seed
  (`builtins.hashString "sha256" "nixfleet-harness-test-seed-2026"`)
  wrapped into a PKCS#8 PrivateKeyInfo via RFC 8410 §7 ASN.1. OpenSSL 3
  cannot accept a caller-supplied seed for `genpkey -algorithm ED25519`
  directly (see openssl/openssl#18333); hand-building the 48-byte DER is
  the cleanest path. Changing the seed string forces a new keypair
  everywhere downstream.
- **Canonicalizer** — pinned `nixfleet-canonicalize` package (serde_jcs
  0.2 per `docs/CONTRACTS.md` §III).

Verify with two evals:

```bash
nix eval --impure \
  --expr '(builtins.getFlake (toString ./.)).checks.x86_64-linux.phase-2-signed-fixture.drvPath'
```

Run the command twice — the two `drvPath` strings MUST match. A
mismatch means one of the inputs has leaked impurity (system time,
randomness, absolute paths) and needs tracing.

## Consumers

All pending. Updated to links as work lands.

- **`tests/harness/scenarios/signed-roundtrip.nix`** (Phase 2 PR(b),
  TODO) — serves `canonical.json` + `canonical.json.sig` from the CP
  stub, mounts `test-trust.json` into the agent microVM, asserts
  verify succeeds and the agent logs `harness-roundtrip-ok:`.
- **`crates/nixfleet-verify-artifact`** (Phase 2 PR(a), Stream C,
  TODO) — thin CLI wrapping `reconciler::verify_artifact`. Receives
  the four files as `--artifact`, `--signature`, `--trust-file`, and
  a derived `--now` / `--freshness-window-secs`.

## Out of scope here

Per `docs/phase-2-entry-spec.md` §9 — the first wire-up deliberately
exercises only one algorithm and one non-rotation trust configuration.
Explicit non-goals for this fixture:

- ECDSA P-256 signatures (unit-tested in `crates/nixfleet-reconciler`).
- Multi-key `previous` rotation (follow-up scenario
  `fleet-harness-signed-rotation-cross-algo`).
- `rejectBefore` compromise switch (scenario-specific).
- Tampered-signature refusal (Checkpoint 2 scenario copies this
  derivation and flips one byte).

Each of those is a sibling derivation that copies this one and changes
one input.
