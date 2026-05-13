# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). Versioning: [Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-05-11

v0.2 is a complete rewrite from v0.1. The architecture inverts the v0.1 trust model: truth now lives in git and signing keys, the control plane is a caching router for already-signed intent, and the agent independently verifies every artefact it is asked to run. The v0.1 ABI is not preserved; v0.2 is a hard break, not an upgrade path.

### Trust model

Truth lives in git and signing keys. CI signs `fleet.resolved.json` and the revocations sidecar with the release key; the binary cache signs closures independently. The control plane holds no signing keys and no operator secrets - it routes already-signed artefacts and is rebuildable from empty state without data loss. Trust roots are declared in the flake (`nixfleet.trust.{ciReleaseKey,atticCacheKey,orgRootKey}`), serialised into a typed `trust.json` as part of fleet.resolved, and consumed by the reconciler's `verify_artifact` decision procedure. Algorithm rotation is supported end-to-end: declare the new key alongside the old, let CI sign with the new key on next release, retire the old key once every host has confirmed against it.

### Operator surface

One umbrella binary, `nixfleet`, replaces the v0.1 zoo. Subcommands: `status`, `rollout trace`, `config init`, `mint-token`, `mint-operator-cert`, `derive-pubkey`. The auditor-facing binaries `nixfleet-canonicalize` and `nixfleet-verify-artifact` stay standalone with lean dependency closures so a third-party regulator can verify signed artefacts without pulling the operator network stack. Ceremony tooling lives in `nixfleet-trust-bootstrap` - mint the offline fleet root and the TPM-bound issuance CA from the operator's keyslots. Per-flag resolution chain on every operator surface: `--flag` > `NIXFLEET_<NAME>` env var > `~/.config/nixfleet/config.toml` > built-in default.

### Runtime + agent

The agent runs on every managed host as a NixOS service module (Linux) or launchd agent (Darwin). Its mTLS client identity is bound to `/etc/ssh/ssh_host_ed25519_key`, not a fresh keypair - the control plane refuses any enrollment or renewal CSR whose pubkey does not match the declared `nixfleet.fleetSchema.hosts.<hostname>.pubkey`. Apply path is pull-based with magic rollback: poll for the target closure, fetch with signature verification, activate, open the post-activation confirm window, auto-revert on silence. Per-host declarative health probes feed the wave-promotion gate. Self-switch resilience: the activation process is queued in a detached transient systemd unit before activation begins, so systemd stopping the agent does not kill the in-flight activation.

### Compliance

Compliance is a release gate, not a scanner. Static gates evaluate every host's `config` at CI time; a failing control aborts the pipeline and no signed artefact is produced. Runtime probes execute post-activation, canonicalise their output, and sign with the host SSH key. The control plane aggregates signed probe outputs and feeds them to the wave-promotion gate - a failing runtime probe blocks promotion and can trigger rollback. The full evidence chain runs from git commit, through CI signature, closure hash, and host-signed probe.

### Build + release

The Rust workspace builds via crane caching. CI signs releases with an HSM-held key; the binary cache signs closures with its own ed25519 key. The control plane fetches the signed revocations sidecar (`revocations.json`) from git on every reconcile tick, replays the verified set, and refuses to mint certs for revoked agents. Prometheus metrics live behind a default-on Cargo feature flag - `cargo build --no-default-features` drops both `metrics` and `metrics-exporter-prometheus` from the dep tree and `/metrics` returns 404.

### v0.3 trajectory

The v0.3 RFC track formalises the next round of trust mechanism. RFC-0004 specifies hardware-rooted trust (TPM-bound issuance CA, EK-quoted attestation). RFC-0005 specifies the trust lifecycle (operator-role hierarchy, rotation runbooks, active host-attestation quarantine, threshold-signed channels). RFC-0006 specifies the freshness-window policy. RFC-0007 specifies air-gapped operation via signed bundles. None of these break the v0.2 wire protocol; all extend it.

## [0.1.0] - 2026-04-19

Initial release.

[Unreleased]: https://github.com/arcanesys/nixfleet/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/arcanesys/nixfleet/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0
