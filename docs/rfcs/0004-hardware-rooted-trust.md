# RFC-0004: Hardware-rooted trust

**Status.** Draft.
**Targets.** v0.3.
**Depends on.** RFC-0001, RFC-0003, ARCHITECTURE.md §4 (trust roots) / §5 (failure cases).
**Scope.** Anchor v0.2's signed-evidence chain in hardware. Move host signing keys into the TPM, bind agenix-style secret decryption to PCR state, add boot measurements as a probe class with the same signature semantics as runtime probes. Out of scope: confidential computing (SEV/TDX), TPM 1.2, ARM SBCs without a TPM (soft-key fallback only).

## 1. Motivation

ARCHITECTURE.md §5 names the residual risk verbatim:

> *Host is compromised (root on the target machine). Attacker can: read secrets decrypted for that host, forge probe outputs signed with that host's key.*

v0.2 relies on the host's SSH host key (`/etc/ssh/ssh_host_ed25519_key`) for both signing probe outputs and decrypting agenix secrets. That key is on disk. Disk extraction or root post-boot grants the attacker the same signing capability the host has - a forged `ComplianceFailureSignedPayload` is indistinguishable from a real one to the offline auditor (`nixfleet-verify-artifact probe`).

Closing this gap is the v0.3 thesis. The trust property RFC-0004 establishes:

> A signed artifact from a host is also a proof that the host's measured boot state matched declared expectations at the moment of signing.

## 2. Design principle

The TPM does not become a trust authority. It becomes a verifier of conditions for cryptographic operation. Every property v0.2 verifies cryptographically continues to be verified cryptographically; the TPM ensures those operations cannot be performed under conditions other than the intended ones.

The four trust roots in ARCHITECTURE.md §4 do not change. What changes is the per-host signing key: it is now generated inside the TPM, sealed against a declared PCR set, and cannot be exported. The control plane gains no new authority.

## 3. What already exists in v0.2

`impls/keyslots/tpm/` ships a working TPM2 keyslot abstraction:

- `nixfleet.keyslots.tpm.keys.<name>` - first-boot oneshot creates a primary key, evicts to a persistent handle, exports the public half.
- `pcrPolicy = [ "0" "2" "4" "7" ]` - bind the auth policy to a chosen PCR set; signing fails on PCR mismatch.
- `algorithm = "ecdsa-p256" | "ed25519"` - both supported. ecdsa-p256 is the realistic default (commodity TPMs rarely implement ed25519).
- Per-key `tpm-sign-<name>` shell wrapper that consumers (CI runner, agent) invoke to sign a file.
- Idempotent across impermanence wipes - re-extracts pubkey from the persisted handle.

RFC-0004 does **not** reimplement any of this. It extends the existing surface with a single concept (host-identity binding) and adds the missing wire and verification machinery (boot evidence, PCR-bound secret recipients, expected-PCR derivation).

## 4. Components

### 4.1 TPM-bound host identity

Add one option to the existing `keyslots.tpm.keys.<name>` schema:

```nix
nixfleet.keyslots.tpm.keys.host-identity = {
  handle      = "0x81010003";
  algorithm   = "ecdsa-p256";
  pcrPolicy   = [ "0" "2" "4" "7" "8" "9" "11" "12" "13" "14" ];
  enrollAsHostIdentity = true;       # NEW - RFC-0004
};
```

When `enrollAsHostIdentity = true`:

1. The keyslot's exported pubkey (`pubkey.raw`) becomes the host's signing identity. `nixfleet.host.signingPubkeyFile` (new contract field on `contracts/host-spec.nix`) resolves to the keyslot's `pubkey.raw` path.
2. The agent uses the keyslot's `tpm-sign-host-identity` wrapper instead of `evidence_signer.rs`'s file-backed ed25519 path. Probe outputs are TPM-signed.
3. The keyslot's pubkey is what `mkFleet` references in `hosts.<name>.pubkey` (existing field per RFC-0001 §2.1, currently a bare OpenSSH-format string - extended to allow either an inline string or `{ source = "tpm-keyslot/host-identity"; }`).

Exactly one keyslot per host may set `enrollAsHostIdentity = true`. mkFleet asserts this at evaluation time.

The host's SSH host key is **not** removed. It continues to anchor agenix decryption (until §4.3 lands) and SSH transport. The TPM-bound key takes over the v0.2 signing role; ssh and agenix continue using the SSH key during the migration window. After §4.3 ships, agenix can opt into TPM unsealing per-secret.

### 4.2 Boot measurement chain

UEFI Secure Boot + `systemd-stub` + `systemd-measure` produce a deterministic PCR trace from firmware through the kernel command line. The closure declaration knows the kernel hash, initrd hash, and cmdline; CI computes the expected PCR set per host and emits it as part of `fleet.resolved.json`.

This is an additive RFC-0001 schema extension (RFC-0001 §4.1 shape, additional optional field per host):

```json
"hosts": {
  "water-plant-01": {
    "system": "x86_64-linux",
    "closureHash": "sha256-...",
    "tags": ["..."],
    "channel": "stable",
    "pubkey": "ssh-ed25519 AAAA...",
    "expectedBootEvidence": {
      "pcrPolicy": { "pcrs": [0,2,4,7,8,9,11,12,13,14], "algorithm": "sha256" },
      "expectedDigest": "sha256:9f4a2e...",
      "firmwareGeneration": 3
    }
  }
}
```

The `expectedDigest` is a deterministic function of the closure's bootable inputs (kernel, initrd, cmdline) and the host's declared `firmwareGeneration`. `mkFleet` produces it; CI signs the whole artifact. Hosts without `expectedBootEvidence` are pre-§4.4 hosts that never enrolled into attestation - verification gating is opt-in per host.

`firmwareGeneration` is a manual integer in `fleet.nix` (`hosts.<name>.firmwareGeneration = 3`). Default `1`. Operator bumps after testing new firmware on a staging host and capturing the new PCR digest. The framework refuses to make firmware drift silent: a host whose measured PCRs disagree with its declared `expectedDigest` is flagged regardless of whether the difference is malicious or a legitimate firmware update.

Manual is the v0.3 pick because it is one line of code and forces a human acknowledgment of every firmware change. Failure mode: an operator who runs a firmware update without bumping `firmwareGeneration` sees every host on that hardware drift into `AttestationDrift` until they bump. Auto-derivation - a capture tool that writes a checked-in `firmware-evidence/<hostname>.json` that `mkFleet` reads - is the natural follow-up to remove the forget-failure mode; it is in §10 open questions and not v0.3 scope.

Tooling: `nix run .#capture-boot-evidence -- --hostname water-plant-01` runs on a staging host, reads its current PCR quote, and emits a fragment ready to paste into `fleet.nix` (or to commit via a follow-up CLI). No mechanism for "trust whatever the host reports" - the operator always reviews.

### 4.3 PCR-bound secret recipients

agenix recipient declarations gain a TPM-unsealing variant. The contract addition lives in `impls/secrets/` (already the home of identity-path resolution per ARCHITECTURE.md §10.4):

```nix
age.secrets.cluster-token = {
  file = ./secrets/cluster-token.age;
  recipients = [
    { type = "host-tpm";
      host = "water-plant-01";
      pcrPolicy = "@boot"; }    # references expectedBootEvidence above
  ];
};
```

`@boot` resolves at evaluation time to the host's declared `expectedBootEvidence.pcrPolicy`. Custom PCR sets (`pcrPolicy = { pcrs = [0 7]; algorithm = "sha256"; }`) are accepted for secrets that need different boot-state binding than the host-identity key.

Encryption produces an age stanza wrapping the secret to a TPM-policy recipient. Decryption succeeds only when the PCR state at unseal time matches the policy. A tampered kernel produces a PCR mismatch, which produces a TPM authorization failure, which produces a decryption failure - the secret never reaches userspace. The control plane never sees plaintext or the unsealing condition.

Implementation note: this requires extending the agenix decryption path. Two options on the table - a small wrapper around `age` that invokes the TPM keyslot's wrapper for stanza decryption, or a `clevis`-style integration. Pick at implementation time; the wire/declaration shape above is what consumers depend on.

### 4.4 Boot-state probe class

The agent collects boot measurements via `tpm2 quote` with a control-plane-issued nonce, signs the quote with the TPM-bound host key (§4.1), and includes it in the checkin payload.

This rides RFC-0003 §4.1 `POST /agent/checkin` - boot state can drift between activations (firmware updates without reboot are rare but possible), and the cost of carrying a fresh quote on every checkin is negligible. The schema extension to `CheckinRequest` (additive, `Option<T>` + `serde(default)` per nixfleet-proto convention):

```rust
pub struct CheckinRequest {
    // ... existing fields ...
    pub boot_evidence: Option<BootEvidence>,   // NEW - RFC-0004
}

pub struct BootEvidence {
    pub pcr_quote: Vec<u8>,        // TPM2_Quote output
    pub pcr_signature: Vec<u8>,    // host-key signature over pcr_quote || nonce
    pub nonce: [u8; 32],            // CP-issued, anti-replay (delivered in prior checkin response)
    pub measured_digest: Digest,    // computed locally from quote
    pub firmware_generation: u32,   // host's declared generation
}
```

The CP-side response (`CheckinResponse`) gains a `next_attestation_nonce: [u8; 32]` field that the agent caches and replays on the next checkin's `boot_evidence.nonce`. Anti-replay is bounded by the freshness window (RFC-0006).

Verification happens twice. Agent-side: a sanity check that the locally-measured digest matches the locally-quoted one (catches local tooling failures, not malice). CP-side: compare `measured_digest` against `expectedBootEvidence.expectedDigest` from the host's `fleet.resolved` entry; verify the host-key signature on the quote+nonce; emit one of three outcomes:

- **`AttestationOK`** - digest matches. Soft-recorded; no action.
- **`AttestationDrift`** - digest mismatches but signature is valid. The host is honestly reporting an unexpected boot state. New `ReportEvent::AttestationDrift { hostname, expected, measured }` (additive wire variant per the RFC-0003 idiom). Triggers RFC-0005 §4 quarantine if persistent.
- **`AttestationInvalid`** - signature does not verify. Host is lying or impersonating. Same `ReportEvent` payload but distinct status; triggers immediate quarantine, not threshold-based.

Hosts with no `expectedBootEvidence` declared in `fleet.resolved` and no `boot_evidence` in their checkin are pre-attestation hosts; the CP records `attestation_status = none` and proceeds normally. Migration is per-host, not all-or-nothing.

### 4.5 Closure-derived expectations

`mkFleet` is extended to produce, per host, the `expectedBootEvidence` block deterministically from:

- The host's `configuration.config.system.build.toplevel` (kernel + initrd + cmdline reachable from there).
- The host's `firmwareGeneration` integer.
- The PCR set declared in `nixfleet.keyslots.tpm.keys.host-identity.pcrPolicy` (single source of truth for which PCRs matter for this host).

This generalizes a property the framework already has: closure hashes are deterministic functions of inputs. Boot-evidence prediction is the same property applied to TPM measurements. Implementation detail - the digest computation may need to call out to a small Rust helper (`nixfleet-pcr-predict`) because pure-Nix PCR prediction for systemd-stub measurements is non-trivial; that helper runs as a derivation builder, not at agent runtime.

## 5. Trust analysis

**Properties added.**

- Extraction-resistant host signing keys: stealing a disk yields no usable signing capability.
- Boot-state proof in the evidence chain: every probe signature now also attests "the boot chain at signing time matched the declared expectation."
- Secret access bound to boot state: a tampered kernel produces a TPM authorization failure before plaintext is reachable.
- Detectable kernel/initrd tampering before secrets are decrypted, before probes are signed.

**Properties not added.**

- Protection against runtime tampering after a successful unsealed boot. Root post-boot operates within the unsealed-key scope until reboot. Mitigation requires confidential computing (AMD SEV / Intel TDX) - a future RFC.
- Protection against TPM-bus physical attacks. Out of scope; well-funded attackers with sustained physical access can attack the bus. Confidential computing again.
- Trust in the TPM manufacturer beyond what the EK certificate verification policy specifies. RFC-0005 picks a default policy.

**Failure cases (per ARCHITECTURE.md §5 idiom).**

- *TPM unavailable on a host.* Enrollment refuses unless `enrollAsHostIdentity` is left unset; the resulting deployment continues using the v0.2 SSH-host-key path. Visible in fleet status as `signingBackend: ssh-host-key` (vs `tpm-keyslot/host-identity`).
- *PCR drift from firmware update.* Operator tests new firmware on a staging host, runs `nix run .#capture-boot-evidence`, bumps `firmwareGeneration`, commits. CI reissues `fleet.resolved.json`. Until then, agents on updated firmware emit `AttestationDrift`; if persistent, RFC-0005 §4 quarantines them.
- *TPM hardware failure.* Host is re-enrolled as a new host (new EK, new keyslot pubkey, new mTLS cert via the existing enrollment flow). The old record is retired. Procedure documented in RFC-0005 §7 rotation runbooks.
- *Operator forgot to declare a legitimate change.* Same as drift: visible, blocking, recoverable. The framework refuses silent acceptance.

## 6. Wire-protocol additions

| Artifact | Addition | Type |
|---|---|---|
| `fleet.resolved.json` host entries | `expectedBootEvidence: Option<...>` | additive (RFC-0001 §4.1, no version bump) |
| `CheckinRequest` | `boot_evidence: Option<BootEvidence>` | additive (RFC-0003 §4.1, no version bump) |
| `CheckinResponse` | `next_attestation_nonce: Option<[u8; 32]>` | additive (RFC-0003 §4.1, no version bump) |
| `ReportEvent` | `AttestationDrift`, `AttestationInvalid` | additive variants (RFC-0003 §4.3) |
| `HostStatusEntry` (CP `/v1/hosts`) | `attestation_status`, `boot_state_age` | additive |

`PROTOCOL_MAJOR_VERSION` does not change. Pre-RFC-0004 agents and CPs interoperate with RFC-0004 components transparently - they simply lack attestation enforcement.

## 7. Migration

Per-host opt-in.

1. Enable `nixfleet.keyslots.tpm` on the host. First-boot generates a `host-identity` keyslot at the declared handle.
2. Capture initial expected-boot-evidence: `nix run .#capture-boot-evidence -- --hostname <h>`. Commit the fragment.
3. Set `enrollAsHostIdentity = true` on the keyslot, change `hosts.<h>.pubkey` to reference the keyslot. Commit.
4. CI rebuilds, signs new `fleet.resolved.json`. Agent on next checkin starts including `boot_evidence`.
5. CP starts logging attestation outcomes; advisory only until the operator promotes to enforcement (via RFC-0005 §4 quarantine policy).

There is no fleet-wide flag day. The framework supports a mixed fleet (some hosts attested, some not) indefinitely - the per-host `expectedBootEvidence` field's presence is the per-host opt-in.

## 8. Work items

- **`expectedBootEvidence` schema + mkFleet derivation.** Schema lands in RFC-0001's evaluation contract; `mkFleet` produces the field for hosts that have declared `enrollAsHostIdentity`. CI's `nixfleet-release` signs over the new field. Deliverable: `fleet.resolved.json` for an attestation-opted host carries a valid `expectedBootEvidence` block; bit-flipping it fails verification.
- **Host-identity keyslot.** `enrollAsHostIdentity` flag + agent's switch from SSH-key signing to TPM-wrapper signing for probe outputs. Deliverable: a host with the flag set produces probe-output signatures that the offline auditor (`nixfleet-verify-artifact probe`) verifies against the TPM-derived pubkey, and an attempt to sign a fake probe with on-disk material is rejected by the same auditor.
- **Boot-evidence collection (advisory).** Agent collects + signs PCR quote on every checkin; CP logs `AttestationOK / Drift / Invalid` to `host_reports` (the existing SQLite table covered by the CP-resident-state recovery profile in docs/design/architecture.md §6). No gating yet. Deliverable: a tampered kernel produces a visible `AttestationDrift` event in fleet status.
- **PCR-bound secret recipients.** agenix-equivalent extension; one supported PCR-binding mechanism implemented end-to-end. Deliverable: a secret encrypted to a host's `@boot` PCR policy fails to decrypt on a tampered boot chain, without any agent-side or CP-side intervention.

Enforcement (boot-evidence as a wave-promotion gate) is the subject of RFC-0005 §4 - kept separate so the mechanism (this RFC) and the policy (lifecycle RFC) ship independently.

## 9. Falsifiable done criteria

1. Disk extraction from a host enrolled with the host-identity keyslot yields no usable signing capability; an attempt to use the on-disk material to sign a fake probe is rejected by `nixfleet-verify-artifact probe` against the host's TPM-derived pubkey.
2. Booting a host with boot-evidence collection active and a modified kernel or initrd produces a PCR mismatch that the CP detects on the next checkin and emits as `AttestationDrift`.
3. A secret encrypted to a host's `@boot` PCR policy fails to decrypt when the boot chain is modified, without operator intervention.
4. A host with TPM hardware failure can be re-enrolled and rejoin the fleet under the documented procedure (RFC-0005 §7) in under 30 minutes.
5. A firmware update that the operator has tested and declared via `firmwareGeneration` rolls out without triggering attestation drift.

## 10. Open questions

- **PCR set defaults.** Proposal `[0 2 4 7 8 9 11 12 13 14]` covers firmware + secure-boot databases + bootloader + kernel + initrd + cmdline. This is what nixfleet#83 already gestures at. Tighter sets are possible (just `[0 7]` for firmware + secure-boot DBs) but lose useful attestation surface. Lean: declared-per-host with a `[0 2 4 7 8 9 11 12 13 14]` default.
- **Auto-derived `firmwareGeneration`.** Manual is v0.3; the natural follow-up is a capture tool that writes a checked-in evidence file `mkFleet` reads, removing the operator-forgot-to-bump failure mode. Out of scope for v0.3 but on the v0.4 shortlist.
- **PCR prediction tooling.** `nixfleet-pcr-predict` as a small Rust derivation builder vs calling out to `systemd-measure` directly. Lean: wrap `systemd-measure` for v0.3, replace with native code only if reproducibility issues appear.
- **agenix integration shape.** Wrapper around `age` vs Clevis-style pluggable backend. Lean: wrapper for v0.3 (smaller surface), Clevis if a customer needs it.
- **ARM SBC compatibility.** Many target-vertical edge devices are ARM without TPM. SSH-host-key fallback exists indefinitely; OP-TEE-backed identity is a separate future RFC, not blocking v0.3.
- **EK certificate verification policy.** Manufacturer-chain vs. self-signed inventory at enrollment time. RFC-0005 §3 picks a default.

## 11. One-sentence summary

**The host's signing key lives in the TPM, the boot chain is measured into the signature, and a tampered host cannot speak in the fleet's evidence chain - the v0.2 trust model with its residual hardware-trust gap closed.**
