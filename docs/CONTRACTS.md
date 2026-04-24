# Boundary contracts

The single authoritative reference for every artifact, key, and format that crosses a stream boundary during v0.2. If it is not listed here, it is not a contract — it is implementation detail that can change without coordination.

Every entry declares:
- **Producer** — the stream/component that emits the artifact.
- **Consumer(s)** — streams/components that read it.
- **Schema/version** — current version and the discipline for evolving it.
- **Verification** — what a consumer must check before trusting the content.

Streams referenced:
- **Stream A** — infra (M70q coordinator, out of tree; lives in `fleet` repo).
- **Stream B** — Nix (this repo's `lib/`, `modules/`, + `nixfleet-compliance`).
- **Stream C** — Rust (this repo's `crates/`).

---

## I. Data contracts

### 1. `fleet.resolved.json`

| | |
|---|---|
| **Producer** | CI (Stream A invokes Stream B's Nix eval) |
| **Consumer** | Control plane, agents (fallback direct fetch) |
| **Schema** | v1 — shape defined in RFC-0001 §4.1 |
| **Canonicalization** | JCS (RFC 8785), see §IV |
| **Signature** | CI release key (see §II #1) |
| **Metadata** | `meta.signedAt` (RFC 3339), `meta.ciCommit`, `meta.schemaVersion` |

**Evolution discipline.** Within v1, fields may be added; consumers MUST ignore unknown fields. Removing or changing the meaning of a field requires `schemaVersion: 2` and a migration window.

**Consumer MUST verify before use:**
1. JCS bytes match the canonicalized payload.
2. Signature verifies against the pinned `nixfleet.trust.ciReleaseKey`.
3. `(now − meta.signedAt) ≤ channel.freshnessWindow`.
4. `meta.schemaVersion` is within the consumer's accepted range.

### 2. Wire protocol (agent ↔ control plane)

| | |
|---|---|
| **Producer/Consumer** | Both agent and CP (Stream C) |
| **Schema** | v1 — RFC-0003 §4 |
| **Transport** | HTTP/2 over TLS 1.3, mTLS mandatory |
| **Version header** | `X-Nixfleet-Protocol: 1` |

**Evolution discipline.** Major version in header; mismatched major = HTTP 400. Additive fields within a major; MUST-ignore-unknown-fields on both sides. Removing a field requires a major bump and dual-version CP support during migration.

### 3. Probe descriptor

| | |
|---|---|
| **Producer** | `nixfleet-compliance` (Stream B) |
| **Consumer** | Agent (Stream C) at runtime |
| **Schema** | Per-control `schema = "<framework>/<version>"` field (e.g. `"anssi-bp028/v1"`) |
| **Payload** | `{ command, args, timeoutSecs, expect, schema }` |

**Evolution discipline.** Each framework+version pair is immutable once shipped. New version = new schema string (`anssi-bp028/v2`); agent ships a handler registry keyed on `(control, schema)`. Controls MAY support multiple schema versions during migration.

### 4. Probe output

| | |
|---|---|
| **Producer** | Agent (executing the probe command) |
| **Consumer** | CP (aggregation), auditor (verification) |
| **Schema** | Declared by the control (§I.3 above) |
| **Canonicalization** | JCS |
| **Signature** | Host SSH ed25519 (see §II #4) |

**Evolution discipline.** Output shape is part of the control declaration — changes go through the control schema version. Signature covers the canonicalized bytes plus `{ control, schema, hostname, bootId, generationHash, ts }`.

### 5. Secret recipient list

| | |
|---|---|
| **Producer** | `fleet.nix` (Stream B) |
| **Consumer** | agenix encryption tooling at commit time; agent at activation |
| **Schema** | agenix-native, pinned by `flake.lock` |

**Evolution discipline.** Pinned to the `agenix` version in `flake.lock`. Upgrading agenix is a coordinated commit that re-encrypts all secrets; treat as a spine-level change, not a routine dependency bump.

### 6. Log / event schema

| | |
|---|---|
| **Producer** | CP (reconciler), agent |
| **Consumer** | Operator queries, auditors reading historical state |
| **Schema** | RFC-0002 §7 — structured event with `logSchemaVersion` field |

**Evolution discipline.** Same as wire protocol — additive within a major, bump on breaking changes. Historical events MUST remain parseable for the declared audit retention window.

---

## II. Trust roots

Four keys. Everything else is derived. For each: **who holds the private key, where the public key is declared, and who verifies.**

### 1. CI release key

| | |
|---|---|
| **Private** | HSM / TPM-backed keyslot on M70q (Stream A) |
| **Public (declared)** | `nixfleet.trust.ciReleaseKey` in `fleet.nix` (Stream B) |
| **Verified by** | CP (on `fleet.resolved` load), optionally agents |
| **Algorithm** | ed25519 |
| **Rotation grace** | `nixfleet.trust.ciReleaseKey.previous` valid for 30 days after rotation |

**Rotation procedure.**
1. Generate new keypair in HSM (Stream A).
2. Commit: set `ciReleaseKey = <new>`, `ciReleaseKey.previous = <old>` in `fleet.nix`.
3. CI starts signing with new key on next build.
4. After 30 days, remove `previous` from `fleet.nix`; old-key-signed artifacts rejected.

**Compromise response.** Immediate: remove compromised key from `fleet.nix`, set `rejectBefore = <timestamp>` (all artifacts signed before that are refused regardless of key). Rebuild CI environment. Sign a fresh fleet.resolved from known-clean CI. Document in `SECURITY.md`.

### 2. Attic cache key

| | |
|---|---|
| **Private** | Attic systemd service on M70q (Stream A) |
| **Public (declared)** | `nixfleet.trust.atticCacheKey` (Stream B) |
| **Verified by** | Agents, before every closure activation |
| **Algorithm** | ed25519 (attic's native format) |
| **Rotation grace** | Re-sign history when possible; otherwise 30-day dual-accept window |

**Rotation procedure.** Regenerate attic key; re-sign cached closures with new key (attic tooling supports this); commit new pubkey + grace window.

### 3. Org root key

| | |
|---|---|
| **Private** | Offline hardware (Yubikey) held by operator |
| **Public (declared)** | `nixfleet.trust.orgRootKey` (Stream B) |
| **Verified by** | CP, when validating enrollment tokens |
| **Algorithm** | ed25519 |
| **Rotation grace** | 90 days; effectively never under normal operation |

**Rotation procedure.** Rare. If it rotates, every bootstrap token generated from the old key becomes invalid — every host re-enrollment requires a new token signed by the new key. Not a routine event.

**Compromise response.** Catastrophic: every enrollment token is potentially forgeable. Revoke old key, issue all hosts new bootstrap tokens, re-enroll fleet. Consider this the equivalent of an "infrastructure rebuild" event.

### 4. Host SSH key

| | |
|---|---|
| **Private** | Per-host `/etc/ssh/ssh_host_ed25519_key` (generated at provision) |
| **Public (declared)** | `fleet.nix` host entry (`hosts.<n>.pubkey`) (Stream B) |
| **Verified by** | Auditor (probe output signatures), CP (mTLS cert binding at enrollment) |
| **Algorithm** | ed25519 (OpenSSH-compatible) |
| **Rotation grace** | Host key change = re-enrollment; no grace |

**Rotation procedure.** If a host's key changes, the old host is considered gone and a new one is being enrolled. Secrets must be re-encrypted for the new recipient; probe-output signatures chain through the boot/generation record.

---

## III. Canonicalization

**JCS (RFC 8785) with a single Rust implementation, byte-identical across all signers and verifiers.**

Producer-side (Stream B's `lib/mkFleet.nix`) MUST emit values that round-trip through JCS losslessly: ints only (no floats), deterministic attr order, no JSON-incompatible types. Consumer-side (Stream C's `bin/nixfleet-canonicalize`) pins the library.

- **Library choice.** TBD — Stream C's first commit must pin one (`serde_jcs` or equivalent) and document it here. Requirements: RFC 8785 conformant, handles all JSON edge cases (Unicode NFC, number precision, key sorting on non-ASCII).
- **Golden-file test.** `tests/fixtures/jcs-golden.json` → `tests/fixtures/jcs-golden.canonical` → known ed25519 signature. Test runs on every CI and fails any subtle drift.
- **Usage.** Every signed artifact (fleet.resolved, probe output) is canonicalized via this single library before signing and before verification. No ad-hoc serializers.

When Stream B needs to produce a JCS-canonical artifact (e.g. CI signing fleet.resolved), it invokes the same Rust canonicalizer via a small shell tool (`nixfleet-canonicalize`). Do not reimplement in Nix or shell.

---

## IV. Control-plane storage purity rule

The control plane's SQLite database exists to cache operational state. Every column MUST satisfy one of:

1. **Derivable from git + agent check-ins.** Documented in a line comment on the column:
   ```sql
   CREATE TABLE hosts (
     hostname TEXT PRIMARY KEY,          -- derivable from: fleet.resolved
     current_gen TEXT,                   -- derivable from: agent check-in
     last_seen_at DATETIME,              -- derivable from: agent check-in
     ...
   );
   ```
2. **Explicitly listed in "accepted data loss."** See below.

**Accepted data loss list** — state that is intentionally not preserved through a control-plane teardown:

| State | Reason | Recovery |
|---|---|---|
| Certificate revocation history | Revocations are operational decisions, not automated. | Operator re-declares revocations after teardown. |
| Per-rollout event log (> 30 days old) | Historical trace, not operational. | Available via log aggregation (§I.6), not CP-internal. |

**Rule.** A new column that is neither derivable nor on the accepted-loss list is a contract violation. It fails the teardown test (`#14`) and must be either removed or moved into the declarative state.

---

## V. Versioning summary

| Contract | Current version | Evolution |
|---|---|---|
| `fleet.resolved.json` | `schemaVersion: 1` | Additive within v1; bump for breaking changes |
| Wire protocol | v1 (header) | Additive within major; dual-support during migration |
| Probe descriptor per framework | `<framework>/v1` per framework | New string for new shape; old shape kept during migration |
| Probe output | Tracked with the control | Same as descriptor |
| Log/event | `logSchemaVersion: 1` | Same pattern as wire protocol |
| Agenix format | Pinned by `flake.lock` | Treat upgrade as spine change |

---

## VI. Non-contracts (explicit)

The following are NOT contracts — they may change without coordination:

- Internal CP SQLite layout (as long as §IV rule holds).
- Internal agent process structure (threads, tokio tasks).
- Internal reconciler intermediate data structures.
- Nix module option defaults (overridable per-host).
- Formatter choices, lint rules.
- Directory layout inside `crates/` beyond crate names.

If something that should be a contract is drifting, propose it as an addition to this document via PR — do not unilaterally stabilize it in code.

---

## VII. Amendment procedure

1. Open a PR that modifies this document.
2. Label it `contract-change`.
3. Review requires a signoff from each stream whose code implements the contract.
4. Merge only after the code change that implements the new contract is ready in the same PR (or a linked follow-up that must land within the same spine milestone).
