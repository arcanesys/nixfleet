# nixfleet-release

**Role.** Producer for `releases/fleet.resolved.json` and its signed sidecars. Runs in CI; enumerates hosts from a consumer flake, builds each, pushes closures to a cache, evaluates the resolved fleet, injects closure hashes, stamps `meta`, canonicalises, calls an external sign hook, optionally writes signed `revocations.json` and per-channel rollout manifests, then atomically writes the artefacts and optionally git-commits / pushes. Carries the producer side of the signed-intent contract: anything an agent will execute starts as bytes that come out of this crate.

**Key types and state machines.** `ReleaseConfig` (assembled by the binary CLI; declares flake dir, sign hook, signature algorithm, release dir, git target, optional revocations attr, optional pin source URL), `HostsSpec` (`Auto` / `AutoExclude` / `Explicit`), `HostKind` (`Nixos` / `Darwin`, drives the flake-attr prefix and build path), `RunOutcome` (`Released { commit_sha, hosts }` or `NoChange`). No long-lived state; the pipeline is a top-level `run(&ReleaseConfig) -> Result<RunOutcome>` and the binary thin-wraps it.

**Surface.** The library exports `run`, `inject_closure_hashes`, `stamp_meta`, `canonicalize_resolved`, `filter_expired_pins`, and `render_commit_message`; the binary is `nixfleet-release` with flags mapping 1:1 onto `ReleaseConfig`. Hook contract is the external `--sign-cmd`: receives canonical bytes on `$NIXFLEET_INPUT`, writes a raw signature to `$NIXFLEET_OUTPUT`; signature algorithm must be `ed25519` or `ecdsa-p256`. Per-host pinning supports `pin.commit` / `pin.expires_at`; non-current-commit pins require `--pin-source-url` so the pipeline can build via flake-ref. `reuse_unchanged_signature` produces byte-stable releases on no-op CI runs.

**Links.**

- Generated rustdoc: [`api/nixfleet_release/`](../../mdbook/src/api.md)
- Relevant RFCs: [RFC-0001](../../rfcs/0001-fleet-nix.md), [RFC-0005](../../rfcs/0005-trust-lifecycle.md)
- Architecture component: [§1.2 CI](../../design/architecture.md#12-continuous-integration-the-intent-signing-oracle), [§3 The main flow](../../design/architecture.md#3-the-main-flow), [§4 The trust flow](../../design/architecture.md#4-the-trust-flow)
