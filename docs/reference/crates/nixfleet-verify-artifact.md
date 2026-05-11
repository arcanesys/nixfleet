# nixfleet-verify-artifact

**Role.** Offline verifier CLI. Given a signed artefact, its raw signature, and a `trust.json`, it answers a single binary question: does this artefact carry a valid signature from a currently-trusted key, signed inside the freshness window? Designed to be the regulator / auditor's standalone tool: depends only on `nixfleet-proto`, `nixfleet-canonicalize`, and `nixfleet-reconciler::verify` - no network, no SQLite, no Tokio. Carries the verification half of the trust contract that CI's signer asserts.

**Key types and state machines.** No public Rust API; the crate is binary-only. Internally routes through `nixfleet_reconciler::verify::{verify_artifact, verify_rollout_manifest, verify_canonical_payload}` and `nixfleet_proto::TrustConfig::active_keys_at(now)` so the same time-aware key-rotation policy that the CP uses applies offline.

**Surface.** Three `clap` subcommands:

- `artifact --artifact <path> --signature <path> --trust-file <path> --now <RFC3339> --freshness-window-secs <secs>` - verify a signed `fleet.resolved.json`.
- `rollout-manifest --manifest <path> --signature <path> --trust-file <path> --now <RFC3339> --freshness-window-secs <secs> --rollout-id <id>` - verify a signed rollout manifest AND check that the on-disk bytes recompute to `--rollout-id` (catches mix-and-match / rename attacks).
- `probe --payload <path> --signature <path> --pubkey <path>` - verify a signed probe-output payload against a host's OpenSSH-format `ssh-ed25519` pubkey.

Exit codes are stable: 0 verified, 1 verify error, 2 argument / I/O / parse error. `schemaVersion` mismatches on `trust.json` produce a typed argument error rather than a silent fallback.

**Links.**

- Generated rustdoc: [`api/nixfleet_verify_artifact/`](../../mdbook/src/api.md)
- Relevant RFCs: [RFC-0001](../../rfcs/0001-fleet-nix.md), [RFC-0005](../../rfcs/0005-trust-lifecycle.md), [RFC-0006](../../rfcs/0006-freshness-window-policy.md)
- Architecture component: [§1.5 Agent](../../design/architecture.md#15-agent-the-actuator), [§4 The trust flow](../../design/architecture.md#4-the-trust-flow)
