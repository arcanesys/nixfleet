# nixfleet-canonicalize

**Role.** Pure deterministic JSON canonicalisation for the signing path. JCS (RFC 8785): sort object keys lexically, normalise number representation, force UTF-8. LOADBEARING - every signer (CI release, agent evidence, CP rebuild) and every verifier routes through this crate; drift here invalidates signatures fleet-wide. Lives in the offline-auditor closure: no reqwest, no tokio, no async runtime, so a third-party regulator can build the verifier standalone without pulling the operator's network stack.

**Key types and state machines.** None public beyond two free functions. Operates on `serde_json::Value` (parsed input) and emits canonical bytes ready for ed25519 / ECDSA signing. No mutable state; no statefulness across calls.

**Surface.** Two surfaces: the library functions `canonicalize(&str) -> Result<String>` (JSON string to JCS-canonical bytes) and `sha256_jcs_hex<T: Serialize>(&T) -> Result<String>` (hex-lowercase SHA-256 of the canonical bytes - the function `nixfleet-reconciler::compute_canonical_hash` and `rolloutId` derivation route through here); plus the `nixfleet-canonicalize` binary that reads JSON on stdin and writes canonical bytes on stdout. Binary exit codes: 0 ok, 1 parse / canonicalise error, 2 I/O error. The binary is the auditor's tool, paired with `nixfleet-verify-artifact` for full signature checking.

**Links.**

- Generated rustdoc: [`api/nixfleet_canonicalize/`](../../api/nixfleet_canonicalize/index.html)
- Relevant RFCs: [RFC-0001](../../rfcs/0001-fleet-nix.md), [RFC-0003](../../rfcs/0003-protocol.md)
- Architecture component: [§1.2 CI](../../design/architecture.md#12-continuous-integration-the-intent-signing-oracle), [§4 The trust flow](../../design/architecture.md#4-the-trust-flow)
