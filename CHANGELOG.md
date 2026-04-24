# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). Versioning: [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `lib.mkFleet` — evaluates a declarative fleet description per RFC-0001 and emits a typed `.resolved` artifact. Every invariant from §4.2 is enforced at eval time: host/channel/policy references, host `configuration` validity, edge DAG, compliance-framework allow-list, and the cross-field `freshnessWindow ≥ 2 × signingIntervalMinutes` relation.
- `lib.withSignature` — helper that CI calls to stamp `meta.signedAt` / `meta.ciCommit` onto a resolved fleet before signing.
- `nixfleet.trust.*` option tree in `modules/_trust.nix` — declares CI release key, attic cache key, and org root key (with rotation grace slots and a compromise `rejectBefore` switch) per `docs/CONTRACTS.md §II`.
- `examples/fleet-homelab/` — working end-to-end example producing a signed-shape `fleet.resolved`.
- `tests/lib/mkFleet/` — eval-only harness with positive fixtures (golden JSON comparison), negative fixtures (expected-failure via `tryEval`), and `_`-prefix filter for shared helpers.

### Changed

- New channel options: `signingIntervalMinutes` (default 60) and `freshnessWindow` (no default — must declare). Existing channel definitions must add these to evaluate.
- New host option: `pubkey` (nullable, OpenSSH-format ed25519). Host entries may still omit it; enrollment-bound hosts MUST set it.
- `fleet.resolved` shape extended with a `meta` attribute (`{schemaVersion, signedAt, ciCommit}`) per `docs/CONTRACTS.md §I #1`. Top-level `schemaVersion: 1` is preserved for RFC-0001 §4.1 backward reference.

## [0.1.0] - 2026-04-19

Initial release.

[Unreleased]: https://github.com/arcanesys/nixfleet/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0
