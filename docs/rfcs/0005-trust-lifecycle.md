# RFC-0005: Trust lifecycle

**Status.** Draft.
**Targets.** v0.3.
**Depends on.** RFC-0001, RFC-0003, RFC-0004, ../design/architecture.md §4.
**Scope.** Specify the lifecycle of every key, credential, and authorization in the v0.2/v0.3 trust model: how each is created, held, used, rotated, and retired. Add (a) EK-bound bootstrap tokens, (b) host-attestation quarantine policy, (c) opt-in threshold-signed channels, (d) tested key-rotation runbooks. Most of this RFC is documentation + small tooling; the only meaningful new mechanism is multi-signer release coordination.

## 1. Motivation

../design/architecture.md §4 describes the trust model statically - four roots, derivation rules, verification posture. What is documented and tested:

- Pre-announced rotation slots (`current` / `previous` / `successor` / `retireAt`) on the trust contract - `contracts/trust.nix` enforces the paired-options invariant.
- Bootstrap tokens with hostname + pubkey-fingerprint scoping, single-use via the `token_replay` SQLite table, signed by the org root key - RFC-0003 §4.5.
- Closure-hash quarantine after activation failure (`ClosureQuarantined` event, agent-side state-dir record).
- Cert revocation via the signed `revocations.json` sidecar replayed into `cert_revocations` on every reconcile tick.

What is missing and what auditors ask for first:

1. How operators physically hold the four root keys.
2. How the org root key is generated, witnessed, and escrowed.
3. How CI uses its release key without holding it directly.
4. How a host's first mTLS cert is bound to its actual hardware (not just the operator's claim about it).
5. What happens to a host that persistently fails attestation (RFC-0004 §4.4) or probes - beyond the existing closure-level quarantine.
6. Tested rotation procedures for each of the four root keys.

The documentation gap is the larger work. The mechanism additions are: EK binding on bootstrap tokens (small), host-attestation quarantine policy (small, reuses cert-lifetime as revocation horizon), threshold-signed channels (the only nontrivial new mechanism).

## 1.5 Trust-root wiring (v0.2 baseline)

The lifecycle work in this RFC sits on top of an existing v0.2 wiring path that carries declared trust roots from the Nix layer to the runtime verify call. That path is the load-bearing assumption every subsequent section makes; this section captures the shape so readers do not need to reconstruct it from source.

Declarations live under `nixfleet.trust.{ciReleaseKey,atticCacheKey,orgRootKey}` in the Nix layer (`modules/contracts/trust.nix`). Each entry is a `KeySlot` with `current`, `previous`, and `rejectBefore` fields - `current` is the active key, `previous` covers the rotation grace window, and `rejectBefore` is the compromise-incident switch that refuses any artifact whose `meta.signedAt` predates the cutoff regardless of which key produced the signature. The CP-host NixOS module (`modules/scopes/nixfleet/_control-plane.nix`) materialises the declared attrset as `/etc/nixfleet/cp/trust.json` at activation time and passes `--trust-file` on the CP binary's command line. Agents follow the same pattern through `/etc/nixfleet/agent/trust.json`. The on-disk file is world-readable because it contains only public material; `schemaVersion: 1` is required at the top level and binaries refuse to start on unknown versions.

At runtime the CP deserialises the file into `proto::TrustConfig`, and on every `fleet.resolved` load calls `slot.active_keys()` to get the `&[TrustedPubkey]` slice handed to `reconciler::verify_artifact`. The verify function iterates the slice and matches on each entry's `algorithm` tag, which is what makes cross-algorithm rotation work end-to-end - the same call site verifies ed25519-signed and ecdsa-p256-signed artifacts as long as both keys are present in the active slot pair. The CP never holds trust private keys; the org root, CI release key, and attic signing key all live with operator hardware or CI signing tooling outside the CP host. Rotation happens declaratively in `fleet.nix` and reaches the CP via the normal nixos-rebuild activation path; no separate trust-state replication channel exists, and the CP is reconstructible from git plus agent check-ins by design (docs/design/contracts.md §IV).

## 2. Design principle

Every authorization in the system has an explicit lifecycle: who creates it, where it lives, how long it lasts, how it is revoked, what happens when it is lost. No silent state, no implicit trust, no procedure that exists only in a single operator's head.

When in doubt: prefer hardware-bound, short-lived, narrow-scope, observably-revocable.

## 3. Operator workflow specification

Three operator roles. Each has a documented hardware requirement, a stated maximum lifetime, and a defined revocation path. Procedures live in `docs/runbooks/` (new directory - documentation work item); ceremony tooling in `tools/keys/` (new directory - documentation work item).

### 3.1 Release operator

- **What they hold.** A YubiKey 5+ enrolled as a release-channel signer (PIV slot 9c, ECDSA P-256 - interoperates with the existing `ciReleaseKey` slot type that already supports `ecdsa-p256`).
- **What they do.** Touch the YubiKey to authorize a release-signing operation. Holds no decryption capability. The CI runner's signing process blocks on operator touch; without it, nothing is signed as a release.
- **Default rotation cadence.** 12 months; new YubiKey enrolled, old removed from `nixfleet.trust.ciReleaseKey` via the existing `current` -> `previous` rotation slot pattern.
- **Loss procedure.** Revoke from `nixfleet.trust.ciReleaseKey.previous` immediately; `successor` becomes `current` if pre-announced, else operator runs an out-of-band rotation ceremony.

### 3.2 Org root operator

- **What they hold.** One Shamir share of the org root key, default 2-of-3 threshold. Each share lives on a hardware token (X25519 key on YubiKey or equivalent - same hardware family as 3.1 but a distinct slot).
- **What they do.** Reconstruct the threshold to sign bootstrap tokens (host enrollment), CI key rotation envelopes, and major trust changes. Org-root signatures are timestamped and committed to a transparency log - for v0.3, an append-only file in the fleet repo (`trust/transparency.log`); future iterations may integrate with an external transparency service.
- **Default rotation cadence.** 24 months for individual shares, on a major incident for the root itself.
- **Single share lost.** Routine; below threshold has no impact. Share is reissued at the next routine ceremony.
- **Threshold lost.** Catastrophic. Re-genesis ceremony + full fleet re-enrollment. Documented as a 24-48 hour recovery procedure. The framework does not pretend this is fast.

### 3.3 Infrastructure operator

- **What they hold.** An SSH key for break-glass access to the coordinator-class hosts.
- **What they do.** Diagnose and recover the CP host itself. Not part of the framework's trust chain - the CP holds no secrets, so SSH access to the CP host has the same blast radius as SSH access to any production NixOS box.
- **Default rotation cadence.** 12 months. Mentioned for completeness because audit will ask.

## 4. Active host-attestation quarantine

When a host's RFC-0004 boot attestation or runtime probes fail persistently, the CP stops issuing fresh mTLS certs to it. Existing certs are short-lived (default 30-day per RFC-0003 §2; renewed at 50% TTL); within one renewal cycle the host falls out of the active fleet view.

This is a **distinct state machine** from closure-hash quarantine (`ClosureQuarantined`). To keep the two clearly separate:

| Mechanism | Trigger | Scope | Origin |
|---|---|---|---|
| `ClosureQuarantined` | Same `closure_hash` fails activation 24h | Per-closure, per-host | v0.2 baseline |
| `HostAttestationQuarantined` | Persistent attestation drift or probe failure | Per-host, all closures | RFC-0005 |

Closure quarantine prevents wasted activation cycles on a known-broken release. Attestation quarantine declares "this host is no longer trusted to act in the fleet." Different lifecycles, different operator surfaces.

### 4.1 Declarative thresholds

```nix
channel.production.attestationQuarantine = {
  attestationFailureThreshold = 3;   # consecutive AttestationDrift / Invalid
  probeFailureThreshold       = 5;   # consecutive non-Pass under enforce mode
  unquarantine                = "manual";   # or "auto-after-N-successes"
  autoUnquarantineSuccesses   = 10;
};
```

Default off per channel. Operators tune thresholds for their environment before promoting to default-on (likely a v0.4 default).

### 4.2 State recovery classification

Per docs/design/architecture.md §6's soft/hard recovery taxonomy (CP-resident state by recovery profile): `HostAttestationQuarantined` is **soft state**. After CP rebuild, repeated attestation failures from the same host re-trigger the quarantine within the threshold window. No signed-artifact replay needed because the trigger is observable from agent inputs.

The quarantine *threshold configuration* is hard state (lives in `fleet.resolved.json`, signed). The quarantine *occurrence record* is soft (rebuilt from continued failures).

### 4.3 Visibility

A quarantined host stays in `/v1/hosts` output as `quarantined since <timestamp>, reason <attestation-drift|probe-failure>`, observable to operators and auditors. The framework prefers visible failure to silent eviction.

### 4.4 Reversibility

For `unquarantine = "manual"`, an operator runs:

```
nix run .#unquarantine-host -- --hostname <h> --reason "<rationale>"
```

(matches the no-big-CLI convention - flake app, not a binary subcommand). The action is logged in the `host_reports` ring with `event_kind = HostUnquarantined`. For `unquarantine = "auto"`, the CP resumes cert issuance after N consecutive successful checkins with passing attestation.

This is policy on top of v0.2's existing short-cert design and §1 cert-revocation infrastructure. No new revocation channel is required - the cert lifetime *is* the revocation horizon, the same way it is for explicit revocations.

## 5. EK-bound bootstrap tokens

Bootstrap tokens already exist (RFC-0003 §4.5, `nixfleet mint-token` subcommand, `BootstrapToken` + `TokenClaims` in `nixfleet-proto`). RFC-0005 extends the token claims with one field:

```rust
pub struct TokenClaims {
    pub hostname: String,
    pub pubkey_fingerprint: String,
    pub expected_ek_fingerprint: Option<String>,  // NEW - RFC-0005
    pub channel: String,
    pub expiry: DateTime<Utc>,
    pub nonce: [u8; 32],
}
```

When `expected_ek_fingerprint` is set:

1. Operator records the host's TPM EK pubkey via OOB tooling when the hardware is unboxed (typed into `fleet.nix` next to the host's other declarations).
2. `nixfleet mint-token` includes the EK fingerprint in the signed claims.
3. The agent's enrollment flow (`POST /v1/enroll`) presents an EK quote alongside the bootstrap token + CSR.
4. The CP verifies: token signature against `orgRootKey`, token unused (existing `token_replay`), CSR pubkey matches `pubkey_fingerprint`, EK in the quote matches `expected_ek_fingerprint`. Mismatch on any of these -> 403 + `EnrollmentFailed` event.

`expected_ek_fingerprint = None` is the v0.2-compatible behavior. Per-host opt-in; once a host enrolls with EK binding, future re-enrollments require a token bound to the same EK (or a fresh token signed after the operator records the new EK following hardware replacement).

This closes "rogue host enrolls itself given a leaked bootstrap token": even with the token, the attacker would need either the original host's TPM (impractical) or a token re-issued after the operator recorded the attacker's EK (operator action, audit-trail visible).

## 6. Threshold-signed channels

Opt-in per channel. A channel declares:

```nix
channel.gov-prod = {
  releaseSigners.threshold = "2-of-3";
  releaseSigners.signers = [
    { name = "alice"; pubkey = "ssh-ed25519 AAAA..."; }
    { name = "bob";   pubkey = "ssh-ed25519 AAAA..."; }
    { name = "charlie"; pubkey = "ssh-ed25519 AAAA..."; }
  ];
};
```

For releases targeting this channel, CI refuses to publish until N hardware-key signatures have been collected on the same canonical bytes.

### 6.1 Mechanism

The current `nixfleet-release` pipeline calls a single `--sign-cmd` hook (../design/architecture.md §11.3). Threshold signing extends this with a multi-process signing session:

1. CI evaluates the fleet, builds closures, canonicalizes `fleet.resolved.json` (existing pipeline through step 7).
2. Instead of calling `--sign-cmd` directly, CI writes a **signing session** to disk: `signing-sessions/<session-id>/canonical.json` plus a `metadata.json` describing which signers must sign, the diff against the previous release, and the build provenance.
3. The signing session is published via the existing CI artifact mechanism (Forgejo Actions artifact, or pushed to a known location).
4. Each signer runs `nix run .#sign-release -- --session <session-id>` on their own workstation. The CLI fetches the session artifact, displays a per-artifact summary (changed hosts, changed compliance frameworks, diff against the previous release on the channel), prompts for YubiKey touch, signs the canonical bytes, uploads the signature back.
5. When N signatures have arrived, a CI follow-up job stitches them into the release artifact (`fleet.resolved.json` + `fleet.resolved.threshold.sig` containing the N signatures + a manifest of which signer signed which).
6. The CP verifies on fetch: each signature in the threshold sig matches a signer in the channel's `releaseSigners`, the count meets the threshold.

The CI release key (the existing `ciReleaseKey`) continues to sign automation-friendly artifacts (revocations, rollout manifests). Threshold signing applies only to `fleet.resolved.json` for opted-in channels.

### 6.2 Failure cases

- *Signer YubiKey lost.* That signer is removed from the channel's `releaseSigners`; the threshold continues with N−1 until a replacement is enrolled. If `N − 1 < threshold`, the channel cannot release until replacement.
- *Signer collusion at threshold.* If N signers collude, they can sign a malicious release. The framework does not prevent this; it makes it visible (every signature is in the transparency log) and rare (hardware-key requirement). Mitigation is organizational, not cryptographic.
- *Signing session expires.* Sessions have a 7-day default expiry (per-channel override). Stale sessions are deleted; CI emits a `SigningSessionExpired` event.

### 6.3 Out of scope for v0.3

Web-based review UX. v0.3 ships the CLI-based session viewer; richer UI is a separate project (the framework's scope stops at "the protocol exists and works from a terminal").

## 7. Key rotation runbooks

One runbook per root key, each tested in a microvm.nix scenario under `tests/harness/scenarios/key-rotation/` (new directory). Runbooks live at `docs/runbooks/<key>-rotation.md` (new directory).

### 7.1 CI release key rotation

Uses the existing `ciReleaseKey.successor` + `retireAt` mechanism. Operator:
1. Generates a new key (typically on a fresh YubiKey).
2. Sets `nixfleet.trust.ciReleaseKey.successor = { algorithm; public; }` and `retireAt = "<RFC3339>"` in the flake. Commits.
3. CP verifiers begin accepting both `current` and `successor` during the overlap window.
4. After `retireAt`, the reconciler emits `Action::RotateTrustRoot`; operator's tooling rotates `current -> previous`, `successor -> current` in the next commit.
5. Old key removed from `previous` after the 30-day grace window per CONTRACTS §II #1.

### 7.2 Attic cache key rotation

1. Generate new attic key on the cache host.
2. Stand up a parallel attic publishing closures under the new key (existing `nixfleet.trust.cacheKeys` already a list - both keys present during overlap).
3. Trigger a CI rebuild that re-pushes all in-use closures to the new cache.
4. Once all hosts have converged on closures signed by the new key, remove the old key from `cacheKeys` and decommission the old attic.

### 7.3 Org root key rotation

Catastrophic procedure (24-48 hour recovery). Re-genesis ceremony per §3.2; new threshold shares distributed to operators; bootstrap tokens going forward signed with new key; old key kept valid for in-flight tokens until expiry; then revoked. All hosts re-enrolled with new bootstrap tokens issued under the new root.

### 7.4 Host TPM key rotation

Operator-initiated re-enrollment (TPM hardware change) or scheduled (every N years per policy).
1. New TPM keyslot generated on the host (existing first-boot flow).
2. Operator captures new pubkey + EK; updates `fleet.nix`; mints new bootstrap token bound to new EK.
3. Host re-enrolls; old mTLS cert revoked via `revocations.json`.
4. Old host record retired in `dispatch_history`.

Each runbook has a microvm scenario (`tests/harness/scenarios/key-rotation/<key>.nix`) that exercises the procedure end-to-end. Scenarios are part of the nightly test fabric, not the per-PR fast suite.

## 8. Trust analysis

**Lifecycle properties added.**

- Each authorization has a documented creation procedure, holding requirement, rotation cadence, and revocation path.
- Bootstrap is single-use, time-bounded, and (with EK binding) hardware-bound.
- Quarantine is observable, reversible, and bounded (one cert renewal cycle to take effect; no new infrastructure needed).
- Threshold signing distributes release authority without introducing a new central authority.

**Failure cases not stated above.**

- *Operator collusion at any threshold.* The framework does not prevent it. Mitigation is organizational (separation of duties), not cryptographic.
- *Quarantine misclassification (host healthy but attestation flapping due to legitimate issue).* Operator unquarantines with rationale logged; investigation drives a fix to the policy or the host. The framework prefers visible failure to silent passing.

## 9. Migration

Most of RFC-0005 is additive documentation. Mechanism additions are per-host or per-channel opt-in:

- **Bootstrap-token EK binding** is opt-in via `expected_ek_fingerprint`. Pre-RFC-0005 tokens (and re-issued tokens for hosts whose hardware was provisioned without EK capture) work unchanged.
- **Host-attestation quarantine** is opt-in per channel via `attestationQuarantine` block. Default off.
- **Threshold-signed channels** are opt-in per channel. Default is single-signer (the existing `ciReleaseKey` flow). Operators opt in by declaring `releaseSigners`.
- **Operator workflow** documentation is the bulk of the work and applies retroactively; no code change required.

## 10. Work items

- **Operator workflow documentation.** Runbooks for the four key types in `docs/runbooks/`; ceremony scripts in `tools/keys/`; hardware compatibility matrix; transparency-log file format. No new Rust code.
- **EK-bound bootstrap tokens.** Token-claims field, `nixfleet mint-token` subcommand flag, EK-quote verification at `/v1/enroll`, single-use enforcement against EK fingerprint.
- **Active host-attestation quarantine.** `attestationQuarantine` channel schema (RFC-0001 additive), CP-side state machine and cert-issuance hook, observable status, `unquarantine-host` flake app, microvm scenario.
- **Threshold-signed channels.** Channel schema additions (`releaseSigners`), signing-session protocol, `sign-release` CLI flake app, CP-side multi-signature verification.
- **Key rotation runbooks tested.** Each rotation procedure has a microvm scenario that runs in the nightly suite.

These work items are largely independent - the documentation tooling unblocks the rest; runbook validation depends on the mechanisms shipping first.

## 11. Falsifiable done criteria

1. Each of the four root keys has a documented rotation runbook, executed end-to-end in a microvm scenario within the last quarter.
2. A bootstrap token's second use is rejected by the CP (existing v0.2 behavior, retained); a token presented by a host whose EK quote does not match `expected_ek_fingerprint` is rejected before any cert is issued.
3. A persistently failing host is removed from the active fleet view within one mTLS cert renewal cycle, observably and reversibly.
4. A threshold-signed release tagged with N−1 signatures is rejected by the CP; the same release with N signatures from valid signers verifies.
5. The org root key can be reconstructed from its threshold shares in an air-gapped session, reproducing the exact public key from the recorded share material plus the documented procedure.
6. An auditor handed a hostname can produce, from records alone, the full enrollment chain: bootstrap token (signed by org root at time T), EK fingerprint, first mTLS cert issuance, all subsequent rotations.

## 12. Open questions

- **Quarantine auto-recovery threshold.** For `unquarantine = "auto"`, what value of N is right? Probably channel-specific; defaults could be 10 for production, 3 for staging.
- **Bootstrap token expiry default.** 7 days for hardware in transit but not yet racked. Per-channel override allowed. Worth tightening for environments with short logistics windows.
- **Threshold-signing session storage.** Pushing signing sessions through the existing Forgejo Actions artifact path is the simplest answer, but it means signers need network reach to the forge. Air-gap channels (RFC-0007) need a different transport - likely a bundle in/out of the air-gap. Defer to RFC-0007 v0.4 cycle.
- **Transparency log target.** Git-tracked append-only file is sufficient for v0.3. v0.4+ may integrate with a public transparency service if customer environments require it.

## 13. One-sentence summary

**Every authorization in the system has a documented birth, life, and death - and a host that lies about its boot state stops being part of the fleet within one cert cycle, observably and reversibly.**
